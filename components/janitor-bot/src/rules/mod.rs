pub mod actions;
pub mod expr;
pub mod matchers;
pub mod schema;

use crate::clients::Clients;
use crate::event::{
    ArgoSyncEvent, BotEvent, CheckRunEvent, CommitStatusEvent, PrEvent, WorkflowEvent,
};
use crate::rules::matchers::MatcherCache;
pub use actions::Action;
use moka::sync::Cache;
use schema::{ActionsDef, RulesFile};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

const RULES_YAML: &str = include_str!(concat!(env!("OUT_DIR"), "/rules.merged.yaml"));

struct ActionGroup<'a> {
    when: Option<&'a str>,
    actions: Vec<&'a schema::ActionDef>,
}

#[derive(Serialize)]
pub struct MatchedRule {
    pub name: String,
    pub dry_run: bool,
    pub actions: Vec<&'static str>,
}

pub type ClockFn = Arc<dyn Fn() -> chrono::DateTime<chrono::Utc> + Send + Sync>;

pub struct RulesOrchestrator {
    rules: RulesFile,
    pr_locks: Cache<(String, String, u64), Arc<Mutex<()>>>,
    workflow_lock: Mutex<()>,
    clock: ClockFn,
}

impl Default for RulesOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl RulesOrchestrator {
    pub fn new() -> Self {
        let rules: RulesFile =
            yaml_serde::from_str(RULES_YAML).expect("rules.yaml deserialization failed");
        Self::from_rules(rules)
    }

    pub fn from_rules(mut rules: RulesFile) -> Self {
        rules.rules.sort_by_key(|r| std::cmp::Reverse(r.priority));
        Self {
            rules,
            pr_locks: Cache::builder()
                .time_to_idle(Duration::from_secs(10 * 60))
                .build(),
            workflow_lock: Mutex::new(()),
            clock: Arc::new(chrono::Utc::now),
        }
    }

    pub fn with_clock(mut self, clock: ClockFn) -> Self {
        self.clock = clock;
        self
    }

    fn now(&self) -> chrono::DateTime<chrono::Utc> {
        (self.clock)()
    }

    pub async fn explain_pr(&self, clients: &Clients, event: &mut PrEvent) -> Vec<MatchedRule> {
        let lock = self.pr_lock(&event.owner, &event.repo, event.pr_number);
        let _guard = lock.lock().await;

        let pr_id = event.pr_number as i64;
        if let Ok(files) = clients
            .forgejo
            .get_pr_changed_files(&event.owner, &event.repo, pr_id)
            .await
        {
            event.changed_files = files;
        }

        let bot_event = BotEvent::ForgejoPr(event);
        self.explain_rules(&bot_event, clients).await
    }

    pub async fn explain_workflow(
        &self,
        clients: &Clients,
        event: &mut WorkflowEvent,
    ) -> Vec<MatchedRule> {
        let _guard = self.workflow_lock.lock().await;

        if event.conclusion == "failure" {
            let result = clients.github.fetch_failed_jobs(&event.jobs_url).await;
            event.failed_jobs_logs = result.logs;
        }

        let bot_event = BotEvent::GitHubWorkflow(event);
        self.explain_rules(&bot_event, clients).await
    }

    pub async fn evaluate_pr(&self, clients: &Clients, event: &mut PrEvent) {
        let lock = self.pr_lock(&event.owner, &event.repo, event.pr_number);
        let _guard = lock.lock().await;

        if event
            .labels
            .iter()
            .any(|l| l.name == crate::command::IGNORE_LABEL)
        {
            tracing::info!(
                pr = event.pr_number,
                "skipping rules: {} label present",
                crate::command::IGNORE_LABEL
            );
            return;
        }

        let pr_id = event.pr_number as i64;
        match clients
            .forgejo
            .get_pr_changed_files(&event.owner, &event.repo, pr_id)
            .await
        {
            Ok(files) => event.changed_files = files,
            Err(e) => tracing::warn!(pr = event.pr_number, "failed to fetch changed files: {e}"),
        }

        let bot_event = BotEvent::ForgejoPr(event);
        self.run_rules(clients, &bot_event).await;
    }

    pub async fn explain_commit_status(
        &self,
        clients: &Clients,
        event: &CommitStatusEvent,
    ) -> Vec<MatchedRule> {
        let bot_event = BotEvent::GitHubCommitStatus(event);
        self.explain_rules(&bot_event, clients).await
    }

    pub async fn evaluate_commit_status(&self, clients: &Clients, event: &CommitStatusEvent) {
        let bot_event = BotEvent::GitHubCommitStatus(event);
        self.run_rules(clients, &bot_event).await;
    }

    pub async fn explain_check_run(
        &self,
        clients: &Clients,
        event: &CheckRunEvent,
    ) -> Vec<MatchedRule> {
        let bot_event = BotEvent::GitHubCheckRun(event);
        self.explain_rules(&bot_event, clients).await
    }

    pub async fn evaluate_check_run(&self, clients: &Clients, event: &CheckRunEvent) {
        let bot_event = BotEvent::GitHubCheckRun(event);
        self.run_rules(clients, &bot_event).await;
    }

    pub async fn explain_argocd_sync(
        &self,
        clients: &Clients,
        event: &ArgoSyncEvent,
    ) -> Vec<MatchedRule> {
        let bot_event = BotEvent::ArgoSync(event);
        self.explain_rules(&bot_event, clients).await
    }

    pub async fn evaluate_argocd_sync(&self, clients: &Clients, event: &ArgoSyncEvent) {
        let bot_event = BotEvent::ArgoSync(event);
        self.run_rules(clients, &bot_event).await;
    }

    pub async fn evaluate_workflow(&self, clients: &Clients, event: &mut WorkflowEvent) {
        let _guard = self.workflow_lock.lock().await;

        if event.conclusion == "failure" {
            let result = clients.github.fetch_failed_jobs(&event.jobs_url).await;
            event.failed_jobs_logs = result.logs;
        }

        let bot_event = BotEvent::GitHubWorkflow(event);
        self.run_rules(clients, &bot_event).await;
    }

    pub async fn run_rule_by_name_unconditionally(
        &self,
        clients: &Clients,
        event: &mut PrEvent,
        action_name: &str,
    ) -> anyhow::Result<()> {
        let rule = self
            .rules
            .rules
            .iter()
            .find(|r| r.name == action_name)
            .ok_or_else(|| anyhow::anyhow!("no rule named `{action_name}`"))?;

        let lock = self.pr_lock(&event.owner, &event.repo, event.pr_number);
        let _guard = lock.lock().await;

        let pr_id = event.pr_number as i64;
        if let Ok(files) = clients
            .forgejo
            .get_pr_changed_files(&event.owner, &event.repo, pr_id)
            .await
        {
            event.changed_files = files;
        }

        let bot_event = BotEvent::ForgejoPr(event);
        let cache = MatcherCache::new();
        let vars = self
            .evaluate_variables(rule, &bot_event, clients, &cache)
            .await;
        self.execute_actions(rule, &vars, clients, &bot_event, false)
            .await;

        Ok(())
    }

    async fn match_rule<'a>(
        &self,
        rule: &schema::RuleDef,
        event: &BotEvent<'a>,
        clients: &Clients,
        cache: &MatcherCache,
    ) -> bool {
        let now = self.now();
        rule.matches.matches(event, rule, clients, cache, now).await
    }

    async fn evaluate_variables<'a>(
        &self,
        rule: &schema::RuleDef,
        event: &BotEvent<'a>,
        clients: &Clients,
        cache: &MatcherCache,
    ) -> HashMap<String, bool> {
        let now = self.now();
        let mut vars = HashMap::new();
        for defined_variable in &rule.variables {
            let result = defined_variable
                .matcher
                .matches(event, rule, clients, cache, now)
                .await;

            tracing::debug!(
                rule = rule.name,
                var = defined_variable.var,
                result,
                "variable evaluated"
            );
            vars.insert(defined_variable.var.clone(), result);
        }
        vars
    }

    async fn explain_rules<'a>(&self, event: &BotEvent<'a>, clients: &Clients) -> Vec<MatchedRule> {
        let mut matched_rules = Vec::new();
        for rule in &self.rules.rules {
            if !rule.enabled.is_active() {
                continue;
            }

            let cache = MatcherCache::new();
            if !self.match_rule(rule, event, clients, &cache).await {
                continue;
            }

            let vars = self.evaluate_variables(rule, event, clients, &cache).await;
            let groups = Self::resolve_action_groups(rule, &vars).await;
            let actions = groups
                .iter()
                .flat_map(|g| g.actions.iter().map(|a| a.to_action().kind()))
                .collect();

            matched_rules.push(MatchedRule {
                name: rule.name.clone(),
                dry_run: rule.enabled.is_dry_run(),
                actions,
            });
        }
        matched_rules
    }

    async fn run_rules<'a>(&self, clients: &Clients, event: &BotEvent<'a>) {
        for rule in &self.rules.rules {
            if !rule.enabled.is_active() {
                continue;
            }

            let cache = MatcherCache::new();
            let eval_start = Instant::now();
            if !self.match_rule(rule, event, clients, &cache).await {
                tracing::debug!(
                    rule = rule.name,
                    eval_ms = eval_start.elapsed().as_millis(),
                    "rule did not match"
                );
                continue;
            }

            let dry_run = rule.enabled.is_dry_run();
            tracing::info!(
                rule = rule.name,
                dry_run,
                eval_ms = eval_start.elapsed().as_millis(),
                "rule matched"
            );

            let vars = self.evaluate_variables(rule, event, clients, &cache).await;
            self.execute_actions(rule, &vars, clients, event, dry_run)
                .await;
        }
    }

    async fn execute_actions<'a>(
        &self,
        rule: &schema::RuleDef,
        vars: &HashMap<String, bool>,
        clients: &Clients,
        event: &BotEvent<'a>,
        dry_run: bool,
    ) {
        let rule_start = Instant::now();
        let groups = Self::resolve_action_groups(rule, vars).await;
        for group in &groups {
            if let Some(when) = group.when {
                tracing::info!(rule = rule.name, when, "executing action group");
            }

            for action_def in &group.actions {
                let action = action_def.to_action();
                if dry_run {
                    tracing::info!(
                        rule = rule.name,
                        when = group.when,
                        action = action.kind(),
                        "[dry-run] would execute action"
                    );
                    continue;
                }

                let action_start = Instant::now();
                action.execute(clients, event).await;
                tracing::info!(
                    rule = rule.name,
                    when = group.when,
                    action = action.kind(),
                    elapsed_ms = action_start.elapsed().as_millis(),
                    "action executed"
                );
            }
        }

        tracing::info!(
            rule = rule.name,
            elapsed_ms = rule_start.elapsed().as_millis(),
            "rule actions complete"
        );
    }

    async fn resolve_action_groups<'a>(
        rule: &'a schema::RuleDef,
        vars: &HashMap<String, bool>,
    ) -> Vec<ActionGroup<'a>> {
        match &rule.actions {
            ActionsDef::Flat(actions) => vec![ActionGroup {
                when: None,
                actions: actions.iter().collect(),
            }],
            ActionsDef::Conditional(groups) => {
                let mut result = Vec::new();
                for group in groups {
                    let expr = expr::parse(&group.when).expect("pre-validated expression");
                    match expr::eval(&expr, vars) {
                        Ok(true) => {
                            result.push(ActionGroup {
                                when: Some(group.when.as_str()),
                                actions: group.run.iter().collect(),
                            });
                        }
                        Ok(false) => {
                            tracing::debug!(
                                rule = rule.name,
                                when = group.when,
                                "group condition not met, skipping"
                            );
                        }
                        Err(e) => {
                            tracing::error!(
                                rule = rule.name,
                                when = group.when,
                                "expression eval error: {e}"
                            );
                        }
                    }
                }
                result
            }
        }
    }

    fn pr_lock(&self, owner: &str, repo: &str, pr: u64) -> Arc<Mutex<()>> {
        self.pr_locks
            .get_with((owner.to_owned(), repo.to_owned(), pr), || {
                Arc::new(Mutex::new(()))
            })
    }
}
