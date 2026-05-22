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
use chrono::Utc;
use moka::sync::Cache;
use schema::{ActionsDef, RulesFile};
use serde::Serialize;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

const RULES_YAML: &str = include_str!(concat!(env!("OUT_DIR"), "/rules.merged.yaml"));

struct ActionGroup<'a> {
    when: Option<&'a str>,
    actions: Vec<&'a schema::ActionDef>,
}

#[derive(Serialize, Default, Clone)]
pub struct MatchedRule {
    pub name: String,
    pub dry_run: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub variables: Vec<ExplainVar>,
    pub action_groups: Vec<ExplainGroup>,
}

#[derive(Serialize, Clone)]
pub struct ExplainVar {
    pub name: String,
    pub value: String,
}

#[derive(Serialize, Clone)]
pub struct ExplainGroup {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub when: Option<String>,
    pub actions: Vec<&'static str>,
    pub ran: bool,
}

const EVAL_LOG_CAPACITY: usize = 200;

#[derive(Serialize, Clone)]
pub struct EvalLogEntry {
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub event_kind: String,
    pub event_key: String,
    pub matched_rules: Vec<MatchedRule>,
    pub elapsed_ms: u128,
}

#[derive(Serialize)]
pub struct RuleSummary {
    pub name: String,
    pub enabled: String,
    pub priority: i32,
    pub depends_on: Vec<String>,
}

pub type ClockFn = Arc<dyn Fn() -> chrono::DateTime<chrono::Utc> + Send + Sync>;

pub struct RulesOrchestrator {
    rules: RulesFile,
    pr_locks: Cache<(String, String, u64), Arc<Mutex<()>>>,
    workflow_lock: Mutex<()>,
    clock: ClockFn,
    eval_log: Mutex<VecDeque<EvalLogEntry>>,
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
        rules.rules = Self::topo_sort(rules.rules);

        Self {
            rules,
            pr_locks: Cache::builder()
                .time_to_idle(Duration::from_secs(10 * 60))
                .build(),
            workflow_lock: Mutex::new(()),
            clock: Arc::new(chrono::Utc::now),
            eval_log: Mutex::new(VecDeque::with_capacity(EVAL_LOG_CAPACITY)),
        }
    }

    pub fn with_clock(mut self, clock: ClockFn) -> Self {
        self.clock = clock;
        self
    }

    fn now(&self) -> chrono::DateTime<chrono::Utc> {
        (self.clock)()
    }

    async fn record_eval(&self, entry: EvalLogEntry) {
        let mut log = self.eval_log.lock().await;
        if log.len() >= EVAL_LOG_CAPACITY {
            log.pop_front();
        }
        log.push_back(entry);
    }

    pub async fn get_eval_log(&self) -> Vec<EvalLogEntry> {
        self.eval_log.lock().await.iter().cloned().collect()
    }

    pub fn rules_summary(&self) -> Vec<RuleSummary> {
        self.rules
            .rules
            .iter()
            .map(|r| RuleSummary {
                name: r.name.clone(),
                enabled: if r.enabled.is_dry_run() {
                    "dry_run".to_string()
                } else if r.enabled.is_active() {
                    "true".to_string()
                } else {
                    "false".to_string()
                },
                priority: r.priority,
                depends_on: r.depends_on.clone(),
            })
            .collect()
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

    async fn enrich_check_run(clients: &Clients, event: &mut CheckRunEvent) {
        if let (Some(run_id), Some((owner, repo))) =
            (event.run_id, event.repository.split_once('/'))
            && let Some(name) = clients.github.workflow_run_name(owner, repo, run_id).await
        {
            event.workflow_name = name;
        }
    }

    pub async fn explain_check_run(
        &self,
        clients: &Clients,
        event: &mut CheckRunEvent,
    ) -> Vec<MatchedRule> {
        Self::enrich_check_run(clients, event).await;
        let bot_event = BotEvent::GitHubCheckRun(event);
        self.explain_rules(&bot_event, clients).await
    }

    pub async fn evaluate_check_run(&self, clients: &Clients, event: &mut CheckRunEvent) {
        Self::enrich_check_run(clients, event).await;
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
    ) -> HashMap<String, expr::Value> {
        let now = self.now();
        let mut vars = HashMap::new();

        for defined_variable in &rule.variables {
            let value = defined_variable
                .matcher
                .eval_value(event, rule, clients, cache, now)
                .await;

            tracing::debug!(
                rule = rule.name,
                var = defined_variable.var,
                value = %value,
                "variable evaluated"
            );
            vars.insert(defined_variable.var.clone(), value);
        }

        vars
    }

    async fn explain_rules<'a>(&self, event: &BotEvent<'a>, clients: &Clients) -> Vec<MatchedRule> {
        let mut matched_rules = Vec::new();
        let mut executed_rules: HashSet<String> = HashSet::new();

        for rule in &self.rules.rules {
            if !rule.enabled.is_active() {
                continue;
            }

            if !self.dependencies_met(rule, &executed_rules) {
                continue;
            }

            let cache = MatcherCache::new();
            if !self.match_rule(rule, event, clients, &cache).await {
                continue;
            }

            let vars = self.evaluate_variables(rule, event, clients, &cache).await;

            let action_groups = Self::explain_action_groups(rule, &vars);

            if action_groups.iter().any(|g| g.ran && !g.actions.is_empty()) {
                executed_rules.insert(rule.name.clone());
            }

            let mut variables: Vec<ExplainVar> = vars
                .iter()
                .map(|(name, value)| ExplainVar {
                    name: name.clone(),
                    value: value.to_string(),
                })
                .collect();
            variables.sort_by(|a, b| a.name.cmp(&b.name));

            matched_rules.push(MatchedRule {
                name: rule.name.clone(),
                dry_run: rule.enabled.is_dry_run(),
                variables,
                action_groups,
            });
        }

        matched_rules
    }

    async fn run_rules<'a>(&self, clients: &Clients, event: &BotEvent<'a>) {
        let overall_start = Instant::now();
        let mut executed_rules: HashSet<String> = HashSet::new();
        let mut matched_rules_log = Vec::new();

        for rule in &self.rules.rules {
            if !rule.enabled.is_active() {
                continue;
            }

            if !self.dependencies_met(rule, &executed_rules) {
                tracing::debug!(rule = rule.name, "skipping: dependencies not met");
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

            let action_groups = Self::explain_action_groups(rule, &vars);

            let had_actions = self
                .execute_actions(rule, &vars, clients, event, dry_run)
                .await;

            let mut variables: Vec<ExplainVar> = vars
                .iter()
                .map(|(name, value)| ExplainVar {
                    name: name.clone(),
                    value: value.to_string(),
                })
                .collect();
            variables.sort_by(|a, b| a.name.cmp(&b.name));

            matched_rules_log.push(MatchedRule {
                name: rule.name.clone(),
                dry_run,
                variables,
                action_groups,
            });

            if had_actions {
                executed_rules.insert(rule.name.clone());
            }
        }

        self.record_eval(EvalLogEntry {
            timestamp: Utc::now(),
            event_kind: event.event_kind().to_string(),
            event_key: event.event_key(),
            matched_rules: matched_rules_log,
            elapsed_ms: overall_start.elapsed().as_millis(),
        })
        .await;
    }

    fn dependencies_met(&self, rule: &schema::RuleDef, executed: &HashSet<String>) -> bool {
        rule.depends_on.iter().all(|dep| executed.contains(dep))
    }

    async fn execute_actions<'a>(
        &self,
        rule: &schema::RuleDef,
        vars: &HashMap<String, expr::Value>,
        clients: &Clients,
        event: &BotEvent<'a>,
        dry_run: bool,
    ) -> bool {
        let rule_start = Instant::now();

        let groups = Self::resolve_action_groups(rule, vars).await;
        let had_actions = groups.iter().any(|g| !g.actions.is_empty());

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

        had_actions
    }

    async fn resolve_action_groups<'a>(
        rule: &'a schema::RuleDef,
        vars: &HashMap<String, expr::Value>,
    ) -> Vec<ActionGroup<'a>> {
        match &rule.actions {
            ActionsDef::Flat(actions) => vec![ActionGroup {
                when: None,
                actions: actions.iter().collect(),
            }],
            ActionsDef::Conditional(groups) => {
                let mut result = Vec::new();

                for group in groups {
                    let parsed = expr::parse(&group.when).expect("pre-validated expression");
                    match expr::eval(&parsed, vars) {
                        Ok(v) => match v.as_bool() {
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
                                    "expression result is not bool: {e}"
                                );
                            }
                        },
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

    fn explain_action_groups(
        rule: &schema::RuleDef,
        vars: &HashMap<String, expr::Value>,
    ) -> Vec<ExplainGroup> {
        match &rule.actions {
            ActionsDef::Flat(actions) => vec![ExplainGroup {
                when: None,
                actions: actions.iter().map(|a| a.to_action().kind()).collect(),
                ran: true,
            }],
            ActionsDef::Conditional(groups) => groups
                .iter()
                .map(|group| {
                    let parsed = expr::parse(&group.when).expect("pre-validated expression");
                    let ran =
                        matches!(expr::eval(&parsed, vars).map(|v| v.as_bool()), Ok(Ok(true)));
                    ExplainGroup {
                        when: Some(group.when.clone()),
                        actions: group.run.iter().map(|a| a.to_action().kind()).collect(),
                        ran,
                    }
                })
                .collect(),
        }
    }

    fn topo_sort(mut rules: Vec<schema::RuleDef>) -> Vec<schema::RuleDef> {
        rules.sort_by_key(|r| std::cmp::Reverse(r.priority));

        let name_to_idx: HashMap<String, usize> = rules
            .iter()
            .enumerate()
            .map(|(i, r)| (r.name.clone(), i))
            .collect();

        let n = rules.len();

        let mut in_degree = vec![0usize; n];
        let mut dependents: Vec<Vec<usize>> = vec![vec![]; n];

        for (i, rule) in rules.iter().enumerate() {
            for dep in &rule.depends_on {
                if let Some(&dep_idx) = name_to_idx.get(dep) {
                    dependents[dep_idx].push(i);
                    in_degree[i] += 1;
                }
            }
        }

        let mut queue: VecDeque<usize> = (0..n).filter(|&i| in_degree[i] == 0).collect();

        let mut order = Vec::with_capacity(n);
        while let Some(idx) = queue.pop_front() {
            order.push(idx);
            for &dep_idx in &dependents[idx] {
                in_degree[dep_idx] -= 1;
                if in_degree[dep_idx] == 0 {
                    queue.push_back(dep_idx);
                }
            }
        }

        let mut sorted: Vec<Option<schema::RuleDef>> = rules.into_iter().map(Some).collect();
        order
            .into_iter()
            .map(|i| sorted[i].take().unwrap())
            .collect()
    }

    fn pr_lock(&self, owner: &str, repo: &str, pr: u64) -> Arc<Mutex<()>> {
        self.pr_locks
            .get_with((owner.to_owned(), repo.to_owned(), pr), || {
                Arc::new(Mutex::new(()))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_rule(name: &str, priority: i32, depends_on: Vec<&str>) -> schema::RuleDef {
        let deps: Vec<String> = depends_on.into_iter().map(String::from).collect();
        let yaml = format!(
            "name: {name}\nenabled: true\npriority: {priority}\nmatches:\n  type: forgejo\nactions: []"
        );
        let mut rule: schema::RuleDef = yaml_serde::from_str(&yaml).unwrap();
        rule.depends_on = deps;
        rule
    }

    fn sorted_names(rules: Vec<schema::RuleDef>) -> Vec<String> {
        RulesOrchestrator::topo_sort(rules)
            .into_iter()
            .map(|r| r.name)
            .collect()
    }

    #[test]
    fn topo_sort_no_dependencies_sorts_by_priority() {
        let rules = vec![
            make_rule("low", 0, vec![]),
            make_rule("high", 10, vec![]),
            make_rule("mid", 5, vec![]),
        ];
        assert_eq!(sorted_names(rules), vec!["high", "mid", "low"]);
    }

    #[test]
    fn topo_sort_dependency_before_dependent() {
        let rules = vec![
            make_rule("child", 10, vec!["parent"]),
            make_rule("parent", 0, vec![]),
        ];
        assert_eq!(sorted_names(rules), vec!["parent", "child"]);
    }

    #[test]
    fn topo_sort_chain() {
        let rules = vec![
            make_rule("c", 10, vec!["b"]),
            make_rule("a", 0, vec![]),
            make_rule("b", 5, vec!["a"]),
        ];
        assert_eq!(sorted_names(rules), vec!["a", "b", "c"]);
    }

    #[test]
    fn topo_sort_independent_rules_keep_priority_order() {
        let rules = vec![
            make_rule("dep", 0, vec![]),
            make_rule("child", 5, vec!["dep"]),
            make_rule("unrelated-high", 20, vec![]),
            make_rule("unrelated-low", 1, vec![]),
        ];
        let names = sorted_names(rules);
        // dep must come before child
        let dep_pos = names.iter().position(|n| n == "dep").unwrap();
        let child_pos = names.iter().position(|n| n == "child").unwrap();
        assert!(dep_pos < child_pos);
        // unrelated-high before unrelated-low (priority)
        let high_pos = names.iter().position(|n| n == "unrelated-high").unwrap();
        let low_pos = names.iter().position(|n| n == "unrelated-low").unwrap();
        assert!(high_pos < low_pos);
    }

    #[test]
    fn topo_sort_multiple_dependencies() {
        let rules = vec![
            make_rule("child", 10, vec!["a", "b"]),
            make_rule("a", 5, vec![]),
            make_rule("b", 0, vec![]),
        ];
        let names = sorted_names(rules);
        let child_pos = names.iter().position(|n| n == "child").unwrap();
        let a_pos = names.iter().position(|n| n == "a").unwrap();
        let b_pos = names.iter().position(|n| n == "b").unwrap();
        assert!(a_pos < child_pos);
        assert!(b_pos < child_pos);
    }

    #[test]
    fn topo_sort_diamond() {
        // a -> b, a -> c, b -> d, c -> d
        let rules = vec![
            make_rule("d", 0, vec!["b", "c"]),
            make_rule("b", 5, vec!["a"]),
            make_rule("c", 5, vec!["a"]),
            make_rule("a", 10, vec![]),
        ];
        let names = sorted_names(rules);
        let a_pos = names.iter().position(|n| n == "a").unwrap();
        let b_pos = names.iter().position(|n| n == "b").unwrap();
        let c_pos = names.iter().position(|n| n == "c").unwrap();
        let d_pos = names.iter().position(|n| n == "d").unwrap();
        assert!(a_pos < b_pos);
        assert!(a_pos < c_pos);
        assert!(b_pos < d_pos);
        assert!(c_pos < d_pos);
    }

    #[test]
    fn topo_sort_single_rule() {
        let rules = vec![make_rule("only", 0, vec![])];
        assert_eq!(sorted_names(rules), vec!["only"]);
    }

    #[test]
    fn topo_sort_empty() {
        let rules: Vec<schema::RuleDef> = vec![];
        assert!(sorted_names(rules).is_empty());
    }

    #[test]
    fn topo_sort_current_rules() {
        let rules: RulesFile =
            yaml_serde::from_str(RULES_YAML).expect("rules.yaml deserialization failed");
        let sorted = RulesOrchestrator::topo_sort(rules.rules);
        let names: Vec<&str> = sorted.iter().map(|r| r.name.as_str()).collect();
        insta::assert_yaml_snapshot!(names);
    }

    #[test]
    fn topo_sort_dependency_overrides_priority() {
        // child has higher priority but must come after parent
        let rules = vec![
            make_rule("child", 100, vec!["parent"]),
            make_rule("parent", 0, vec![]),
        ];
        assert_eq!(sorted_names(rules), vec!["parent", "child"]);
    }
}
