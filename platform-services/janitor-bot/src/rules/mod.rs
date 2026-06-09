pub mod actions;
pub mod expr;
pub mod matchers;
pub mod schema;

use crate::clients::Clients;
use crate::event::{
    ArgoSyncEvent, BotEvent, CheckRunEvent, CommitStatusEvent, PrEvent, PushEvent, RawRequest,
    WorkflowEvent,
};
use crate::metrics;
use crate::rules::matchers::{Combinator, Matcher, ResourceCache};
pub use actions::Action;
use chrono::Utc;
use moka::sync::Cache;
use schema::{ActionDef, LabelSpec, RulesFile};
use serde::Serialize;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use tracing::Instrument;

const RULES_YAML: &str = include_str!(concat!(env!("OUT_DIR"), "/rules.merged.yaml"));

fn resolve_in_matcher(
    m: &mut Matcher,
    locals: &HashMap<String, Matcher>,
    globals: &HashMap<String, Matcher>,
    visiting: &mut HashSet<String>,
) -> Result<(), String> {
    match m {
        Matcher::Ref(r) => {
            let name = r.name.clone();
            if !visiting.insert(name.clone()) {
                return Err(format!("cycle detected resolving check `{name}`"));
            }
            let check = match name.strip_prefix("global.") {
                Some(global_name) => globals
                    .get(global_name)
                    .ok_or_else(|| format!("unknown global check `{global_name}`"))?,
                None => locals
                    .get(&name)
                    .ok_or_else(|| format!("unknown check `{name}`"))?,
            };
            let mut resolved = check.clone();
            resolve_in_matcher(&mut resolved, locals, globals, visiting)?;
            visiting.remove(&name);
            *m = resolved;
        }
        Matcher::Combinator(c) => match c {
            Combinator::All(ms) | Combinator::Any(ms) => {
                for child in ms {
                    resolve_in_matcher(child, locals, globals, visiting)?;
                }
            }
            Combinator::Not(inner) => resolve_in_matcher(inner, locals, globals, visiting)?,
        },
        Matcher::Leaf(_) | Matcher::LeafExpr(_) => {}
    }
    Ok(())
}

#[derive(Serialize, Default, Clone)]
pub struct MatchedRule {
    pub name: String,
    pub dry_run: bool,
    pub actions: Vec<ExplainAction>,
}

#[derive(Serialize, Clone)]
pub struct ExplainAction {
    pub action: &'static str,
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
            yaml_serde::from_str(RULES_YAML).expect("config.yaml deserialization failed");
        Self::from_rules(rules)
    }

    pub fn from_rules(mut rules: RulesFile) -> Self {
        Self::resolve_checks(&mut rules).expect("check resolution failed");
        Self::apply_label_colors(&mut rules);
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

    /// Watched repos as `(owner, repo)` pairs, parsed from `owner/repo` slugs.
    pub fn watch_repos(&self) -> impl Iterator<Item = (&str, &str)> {
        self.rules
            .repos
            .iter()
            .filter(|r| r.watched)
            .filter_map(|r| r.repo.split_once('/'))
    }

    /// Template vars resolving a GitHub mirror back to its Forgejo repo.
    /// For a GitHub event whose `repository` matches a configured `github_repo`,
    /// exposes `mirror.owner` and `mirror.repo` for the Forgejo target.
    fn mirror_vars(&self, event: &BotEvent) -> HashMap<&'static str, String> {
        let mut vars = HashMap::new();
        let Some(github_repo) = event.template_vars().get("repository").cloned() else {
            return vars;
        };
        if let Some((owner, repo)) = self
            .rules
            .repos
            .iter()
            .find(|r| r.github_repo.as_deref() == Some(github_repo.as_str()))
            .and_then(|r| r.repo.split_once('/'))
        {
            vars.insert("mirror.owner", owner.to_string());
            vars.insert("mirror.repo", repo.to_string());
        }
        vars
    }

    pub fn rules_summary(&self) -> Vec<RuleSummary> {
        self.rules
            .rules
            .iter()
            .map(|r| RuleSummary {
                name: r.name.clone(),
                enabled: if r.dry_run {
                    "dry_run".to_string()
                } else if !r.disabled.is_disabled() {
                    "true".to_string()
                } else {
                    "false".to_string()
                },
                priority: r.priority,
                depends_on: r.depends_on.clone(),
            })
            .collect()
    }

    #[tracing::instrument(skip_all, fields(owner = %event.owner, repo = %event.repo, pr = event.pr_number))]
    pub async fn explain_pr(&self, clients: &Clients, event: &mut PrEvent) -> Vec<MatchedRule> {
        let lock = self.pr_lock(&event.owner, &event.repo, event.pr_number);
        let _guard = lock.lock().await;

        let bot_event = BotEvent::ForgejoPr(event);
        self.explain_rules(&bot_event, clients).await
    }

    pub async fn explain_and_evaluate_pr(
        &self,
        clients: &Clients,
        event: &mut PrEvent,
    ) -> Vec<MatchedRule> {
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

            return Vec::new();
        }

        let bot_event = BotEvent::ForgejoPr(event);

        let cache = ResourceCache::new();
        let resources = self.collect_resources();
        cache.prefetch(clients, &bot_event, &resources).await;

        let matched = self
            .explain_rules_with_cache(&bot_event, clients, &cache)
            .await;
        self.run_rules_with_cache(clients, &bot_event, &cache, None)
            .await;

        matched
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

    #[tracing::instrument(skip_all, fields(owner = %event.owner, repo = %event.repo, pr = event.pr_number, action = %event.action))]
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

        let bot_event = BotEvent::ForgejoPr(event);
        self.run_rules(clients, &bot_event, None).await;
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
        self.run_rules(clients, &bot_event, None).await;
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
        self.run_rules(clients, &bot_event, None).await;
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
        self.run_rules(clients, &bot_event, None).await;
    }

    pub async fn explain_push(&self, clients: &Clients, event: &PushEvent) -> Vec<MatchedRule> {
        let bot_event = BotEvent::GitHubPush(event);
        self.explain_rules(&bot_event, clients).await
    }

    pub async fn evaluate_push(
        &self,
        clients: &Clients,
        event: &PushEvent,
        raw: &RawRequest,
    ) {
        let bot_event = BotEvent::GitHubPush(event);
        self.run_rules(clients, &bot_event, Some(raw)).await;
    }

    pub async fn evaluate_workflow(&self, clients: &Clients, event: &mut WorkflowEvent) {
        let _guard = self.workflow_lock.lock().await;

        if event.conclusion == "failure" {
            let result = clients.github.fetch_failed_jobs(&event.jobs_url).await;
            event.failed_jobs_logs = result.logs;
        }

        let bot_event = BotEvent::GitHubWorkflow(event);
        self.run_rules(clients, &bot_event, None).await;
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

        let bot_event = BotEvent::ForgejoPr(event);

        let cache = ResourceCache::new();
        let resources = self.collect_resources();
        cache.prefetch(clients, &bot_event, &resources).await;

        self.execute_actions(rule, clients, &bot_event, &cache, false, None)
            .await;

        Ok(())
    }

    async fn match_rule<'a>(
        &self,
        rule: &schema::RuleDef,
        event: &BotEvent<'a>,
        clients: &Clients,
        cache: &ResourceCache,
    ) -> bool {
        let now = self.now();
        rule.when.matches(event, rule, clients, cache, now).await
    }

    async fn explain_rules<'a>(&self, event: &BotEvent<'a>, clients: &Clients) -> Vec<MatchedRule> {
        let cache = ResourceCache::new();
        let resources = self.collect_resources();
        cache.prefetch(clients, event, &resources).await;

        self.explain_rules_with_cache(event, clients, &cache).await
    }

    async fn explain_rules_with_cache<'a>(
        &self,
        event: &BotEvent<'a>,
        clients: &Clients,
        cache: &ResourceCache,
    ) -> Vec<MatchedRule> {
        let mut matched_rules = Vec::new();
        let mut executed_rules: HashSet<String> = HashSet::new();

        for rule in &self.rules.rules {
            if rule.disabled.is_disabled() {
                continue;
            }

            if !self.dependencies_met(rule, &executed_rules) {
                tracing::debug!(rule = rule.name, "explain: skipping (deps not met)");
                continue;
            }

            let rule_span = tracing::info_span!(
                "rule.explain",
                otel.name = format!("rule.explain: {}", rule.name),
                rule = rule.name,
                event = event.event_kind(),
            );
            let _enter = rule_span.enter();

            let eval_start = Instant::now();
            if !self.match_rule(rule, event, clients, cache).await {
                tracing::debug!(
                    rule = rule.name,
                    eval_ms = eval_start.elapsed().as_millis(),
                    "explain: rule did not match"
                );
                continue;
            }
            tracing::debug!(
                rule = rule.name,
                eval_ms = eval_start.elapsed().as_millis(),
                "explain: rule matched"
            );

            let actions = self.explain_action_runs(rule, event, clients, cache).await;

            if actions.iter().any(|a| a.ran) {
                executed_rules.insert(rule.name.clone());
            }

            matched_rules.push(MatchedRule {
                name: rule.name.clone(),
                dry_run: rule.dry_run,
                actions,
            });
        }

        matched_rules
    }

    fn collect_resources(&self) -> HashSet<matchers::Resource> {
        let mut resources = HashSet::new();
        for rule in &self.rules.rules {
            if rule.disabled.is_disabled() {
                continue;
            }
            resources.extend(rule.when.requires());
            for group in &rule.actions {
                if let Some(when) = &group.when {
                    resources.extend(when.requires());
                }
            }
        }
        resources
    }

    async fn run_rules<'a>(
        &self,
        clients: &Clients,
        event: &BotEvent<'a>,
        raw: Option<&RawRequest>,
    ) {
        let cache = ResourceCache::new();
        let resources = self.collect_resources();
        cache.prefetch(clients, event, &resources).await;

        self.run_rules_with_cache(clients, event, &cache, raw).await;
    }

    async fn run_rules_with_cache<'a>(
        &self,
        clients: &Clients,
        event: &BotEvent<'a>,
        cache: &ResourceCache,
        raw: Option<&RawRequest>,
    ) {
        let overall_start = Instant::now();
        let mut executed_rules: HashSet<String> = HashSet::new();
        let mut matched_rules_log = Vec::new();

        for rule in &self.rules.rules {
            if rule.disabled.is_disabled() {
                continue;
            }

            if !self.dependencies_met(rule, &executed_rules) {
                tracing::debug!(rule = rule.name, "skipping: dependencies not met");
                continue;
            }

            let rule_span = tracing::info_span!(
                "rule.evaluate",
                otel.name = format!("rule.evaluate: {}", rule.name),
                rule = rule.name,
                event = event.event_kind(),
                event_key = event.event_key(),
            );
            let _enter = rule_span.enter();

            let eval_start = Instant::now();
            if !self.match_rule(rule, event, clients, cache).await {
                tracing::debug!(
                    rule = rule.name,
                    eval_ms = eval_start.elapsed().as_millis(),
                    "rule did not match"
                );

                continue;
            }

            let dry_run = rule.dry_run;
            tracing::info!(
                rule = rule.name,
                dry_run,
                eval_ms = eval_start.elapsed().as_millis(),
                "rule matched"
            );

            let actions_log = self
                .execute_actions(rule, clients, event, cache, dry_run, raw)
                .await;

            let had_ran = actions_log.iter().any(|a| a.ran);

            matched_rules_log.push(MatchedRule {
                name: rule.name.clone(),
                dry_run,
                actions: actions_log,
            });

            if had_ran {
                executed_rules.insert(rule.name.clone());
            }
        }

        let elapsed = overall_start.elapsed();
        let rules_matched = matched_rules_log.len();

        self.record_eval(EvalLogEntry {
            timestamp: Utc::now(),
            event_kind: event.event_kind().to_string(),
            event_key: event.event_key(),
            matched_rules: matched_rules_log,
            elapsed_ms: elapsed.as_millis(),
        })
        .await;

        metrics::record_evaluation(event.event_kind(), rules_matched, elapsed);
    }

    fn dependencies_met(&self, rule: &schema::RuleDef, executed: &HashSet<String>) -> bool {
        rule.depends_on.iter().all(|dep| executed.contains(dep))
    }

    async fn execute_actions<'a>(
        &self,
        rule: &schema::RuleDef,
        clients: &Clients,
        event: &BotEvent<'a>,
        cache: &ResourceCache,
        dry_run: bool,
        raw: Option<&RawRequest>,
    ) -> Vec<ExplainAction> {
        let rule_start = Instant::now();
        let now = self.now();
        let mut log = Vec::new();
        let extra_vars = self.mirror_vars(event);

        for group in &rule.actions {
            let gate_ok = match &group.when {
                None => true,
                Some(when) => when.matches(event, rule, clients, cache, now).await,
            };

            if !gate_ok {
                tracing::debug!(rule = rule.name, "skipping action group: when: not met");
                for action_def in &group.run {
                    log.push(ExplainAction {
                        action: action_def.to_action().kind(),
                        ran: false,
                    });
                }
                continue;
            }

            for action_def in &group.run {
                let action = action_def.to_action();

                if dry_run {
                    tracing::info!(
                        rule = rule.name,
                        action = action.kind(),
                        "[dry-run] would execute action"
                    );
                    log.push(ExplainAction {
                        action: action.kind(),
                        ran: true,
                    });
                    continue;
                }

                let action_start = Instant::now();
                let action_span = tracing::info_span!(
                    "action.execute",
                    otel.name = format!("action: {}", action.kind()),
                    rule = rule.name,
                    action = action.kind(),
                );
                action
                    .execute(clients, event, cache, raw, &extra_vars)
                    .instrument(action_span)
                    .await;
                let action_elapsed = action_start.elapsed();
                metrics::record_action(&rule.name, action.kind(), true);
                tracing::info!(
                    rule = rule.name,
                    action = action.kind(),
                    elapsed_ms = action_elapsed.as_millis(),
                    "action executed"
                );
                log.push(ExplainAction {
                    action: action.kind(),
                    ran: true,
                });
            }
        }

        tracing::info!(
            rule = rule.name,
            elapsed_ms = rule_start.elapsed().as_millis(),
            "rule actions complete"
        );

        log
    }

    async fn explain_action_runs<'a>(
        &self,
        rule: &schema::RuleDef,
        event: &BotEvent<'a>,
        clients: &Clients,
        cache: &ResourceCache,
    ) -> Vec<ExplainAction> {
        let now = self.now();
        let mut out = Vec::new();
        for group in &rule.actions {
            let gate_ok = match &group.when {
                None => true,
                Some(when) => when.matches(event, rule, clients, cache, now).await,
            };
            for action_def in &group.run {
                out.push(ExplainAction {
                    action: action_def.to_action().kind(),
                    ran: gate_ok,
                });
            }
        }
        out
    }

    fn resolve_checks(rules: &mut RulesFile) -> Result<(), String> {
        let globals = rules.checks.clone();
        for rule in &mut rules.rules {
            let locals = rule.checks.clone();
            let mut visiting = HashSet::new();
            resolve_in_matcher(&mut rule.when, &locals, &globals, &mut visiting)
                .map_err(|e| format!("rule `{}`: {e}", rule.name))?;
            for group in &mut rule.actions {
                if let Some(when) = &mut group.when {
                    resolve_in_matcher(when, &locals, &globals, &mut visiting)
                        .map_err(|e| format!("rule `{}`: {e}", rule.name))?;
                }
            }
        }
        Ok(())
    }

    fn apply_label_colors(rules: &mut RulesFile) {
        let registry = &rules.label_colors;
        if registry.is_empty() {
            return;
        }
        let upgrade = |label: &mut LabelSpec| {
            if let LabelSpec::Name(name) = label
                && let Some(color) = registry.get(name)
            {
                *label = LabelSpec::WithColor {
                    name: name.clone(),
                    color: color.clone(),
                };
            }
        };
        for rule in &mut rules.rules {
            for group in &mut rule.actions {
                for action in &mut group.run {
                    match action {
                        ActionDef::AddLabels { labels, .. } => labels.iter_mut().for_each(upgrade),
                        ActionDef::CreateIssue { labels, .. } => {
                            labels.iter_mut().for_each(upgrade)
                        }
                        _ => {}
                    }
                }
            }
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
            "name: {name}\npriority: {priority}\nwhen:\n  type: event\n  kind: pr\n  source: forgejo\nactions: []"
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
            yaml_serde::from_str(RULES_YAML).expect("config.yaml deserialization failed");
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

    fn load(yaml: &str) -> Result<RulesFile, String> {
        let mut f: RulesFile = yaml_serde::from_str(yaml).map_err(|e| e.to_string())?;
        RulesOrchestrator::resolve_checks(&mut f)?;
        Ok(f)
    }

    #[test]
    fn resolve_local_check() {
        let yaml = r#"
rules:
  - name: r
    checks:
      in_window:
        type: time_of_day
        tz: UTC
        after: "09:00"
        before: "17:00"
    when:
      all:
        - type: event
          kind: pr
          source: forgejo
        - ref: in_window
    actions: []
"#;
        let f = load(yaml).unwrap();
        match &f.rules[0].when {
            Matcher::Combinator(Combinator::All(ms)) => {
                assert!(matches!(
                    &ms[1],
                    Matcher::Leaf(matchers::LeafMatcher::TimeOfDay { .. })
                ));
            }
            _ => panic!("expected All"),
        }
    }

    #[test]
    fn resolve_global_check_inside_action_when() {
        let yaml = r#"
checks:
  is_renovate:
    type: author
    value: renovate
rules:
  - name: r
    when:
      type: event
      kind: pr
      source: forgejo
    actions:
      - when:
          ref: global.is_renovate
        run:
          - type: approve
"#;
        let f = load(yaml).unwrap();
        let group = &f.rules[0].actions[0];
        assert!(matches!(
            group.when.as_ref().unwrap(),
            Matcher::Leaf(matchers::LeafMatcher::Author { .. })
        ));
    }

    #[test]
    fn check_can_reference_another_check() {
        let yaml = r#"
checks:
  weekday_evening:
    all:
      - type: time_of_day
        tz: UTC
        after: "17:00"
        before: "22:00"
        weekdays_only: true
rules:
  - name: r
    checks:
      gate:
        all:
          - ref: global.weekday_evening
          - type: bot.has_approved
    when:
      ref: gate
    actions: []
"#;
        let f = load(yaml).unwrap();
        match &f.rules[0].when {
            Matcher::Combinator(Combinator::All(ms)) => assert_eq!(ms.len(), 2),
            _ => panic!("expected All"),
        }
    }

    #[test]
    fn missing_local_ref_errors() {
        let yaml = r#"
rules:
  - name: r
    when:
      ref: nope
    actions: []
"#;
        let err = load(yaml).unwrap_err();
        assert!(err.contains("unknown check `nope`"), "got: {err}");
    }

    #[test]
    fn missing_global_ref_errors() {
        let yaml = r#"
rules:
  - name: r
    when:
      ref: global.nope
    actions: []
"#;
        let err = load(yaml).unwrap_err();
        assert!(err.contains("unknown global check `nope`"), "got: {err}");
    }

    #[test]
    fn cyclic_checks_errors() {
        let yaml = r#"
rules:
  - name: r
    checks:
      a: { ref: b }
      b: { ref: a }
    when:
      ref: a
    actions: []
"#;
        let err = load(yaml).unwrap_err();
        assert!(err.contains("cycle"), "got: {err}");
    }
}
