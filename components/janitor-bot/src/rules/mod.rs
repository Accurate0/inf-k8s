pub mod actions;
pub mod expr;
pub mod matchers;
pub mod schema;
pub mod validate;

use crate::event::{BotEvent, PrEvent, WorkflowEvent};
use crate::forgejo::ForgejoClient;
use crate::github::GitHubClient;
use crate::rules::matchers::MatcherCache;
pub use actions::Action;
use moka::sync::Cache;
use schema::{ActionsDef, RulesFile};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

struct RuleEvaluation {
    matched: bool,
    vars: HashMap<String, bool>,
}

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

impl RulesOrchestrator {
    pub fn from_rules(rules: RulesFile) -> Self {
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

    pub async fn explain_pr(
        &self,
        client: &ForgejoClient,
        event: &mut PrEvent,
    ) -> Vec<MatchedRule> {
        let lock = self.pr_lock(&event.owner, &event.repo, event.pr_number);
        let _guard = lock.lock().await;

        let pr_id = event.pr_number as i64;
        if let Ok(files) = client
            .get_pr_changed_files(&event.owner, &event.repo, pr_id)
            .await
        {
            event.changed_files = files;
        }

        let bot_event = BotEvent::ForgejoPr(event);
        let mut matched_rules = Vec::new();
        for rule in &self.rules.rules {
            if !rule.enabled.is_active() {
                continue;
            }

            let eval = self.evaluate_rule(rule, &bot_event, client).await;
            if eval.matched {
                let groups = Self::resolve_action_groups(rule, &eval.vars).await;
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
        }
        matched_rules
    }

    pub async fn explain_workflow(
        &self,
        client: &ForgejoClient,
        github_client: &GitHubClient,
        event: &mut WorkflowEvent,
    ) -> Vec<MatchedRule> {
        let _guard = self.workflow_lock.lock().await;

        if event.conclusion == "failure" {
            let result = github_client.fetch_failed_jobs(&event.jobs_url).await;
            event.failed_jobs_logs = result.logs;
        }

        let bot_event = BotEvent::GitHubWorkflow(event);
        let mut matched_rules = Vec::new();
        for rule in &self.rules.rules {
            if !rule.enabled.is_active() {
                continue;
            }

            let eval = self.evaluate_rule(rule, &bot_event, client).await;
            if eval.matched {
                let groups = Self::resolve_action_groups(rule, &eval.vars).await;
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
        }
        matched_rules
    }

    pub async fn evaluate_pr(&self, client: &ForgejoClient, event: &mut PrEvent) {
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
        match client
            .get_pr_changed_files(&event.owner, &event.repo, pr_id)
            .await
        {
            Ok(files) => event.changed_files = files,
            Err(e) => tracing::warn!(pr = event.pr_number, "failed to fetch changed files: {e}"),
        }

        let bot_event = BotEvent::ForgejoPr(event);
        self.run_rules(client, &bot_event).await;
    }

    pub async fn evaluate_workflow(
        &self,
        client: &ForgejoClient,
        github_client: &GitHubClient,
        event: &mut WorkflowEvent,
    ) {
        let _guard = self.workflow_lock.lock().await;

        if event.conclusion == "failure" {
            let result = github_client.fetch_failed_jobs(&event.jobs_url).await;
            event.failed_jobs_logs = result.logs;
        }

        let bot_event = BotEvent::GitHubWorkflow(event);
        self.run_rules(client, &bot_event).await;
    }

    async fn run_rules<'a>(&self, client: &ForgejoClient, event: &BotEvent<'a>) {
        for rule in &self.rules.rules {
            if !rule.enabled.is_active() {
                continue;
            }

            let eval_start = Instant::now();
            let eval = self.evaluate_rule(rule, event, client).await;
            let eval_ms = eval_start.elapsed().as_millis();
            if !eval.matched {
                tracing::debug!(rule = rule.name, eval_ms, "rule did not match");
                continue;
            }

            let dry_run = rule.enabled.is_dry_run();
            tracing::info!(rule = rule.name, dry_run, eval_ms, "rule matched");

            let rule_start = Instant::now();
            let groups = Self::resolve_action_groups(rule, &eval.vars).await;
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
                    action.execute(client, event).await;
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
    }

    async fn evaluate_rule<'a>(
        &self,
        rule: &'a schema::RuleDef,
        event: &BotEvent<'a>,
        client: &ForgejoClient,
    ) -> RuleEvaluation {
        let cache = MatcherCache::new();
        let now = self.now();
        let matched = rule.matches.matches(event, client, &cache, now).await;

        let mut vars = HashMap::new();
        for defined_variable in &rule.variables {
            let result = defined_variable
                .matcher
                .matches(event, client, &cache, now)
                .await;

            tracing::debug!(
                rule = rule.name,
                var = defined_variable.var,
                result,
                "variable evaluated"
            );
            vars.insert(defined_variable.var.clone(), result);
        }

        RuleEvaluation { matched, vars }
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
