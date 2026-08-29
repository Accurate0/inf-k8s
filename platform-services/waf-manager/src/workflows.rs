use crate::config::{Combinator, Config, Enabled, LeafMatcher, Matcher, Span, WorkflowDef};
use crate::controller::Context;
use crate::crd::{WafBlock, WafBlockSpec};
use crate::error::Result;
use crate::loki::{Loki, RuleHit, UriHit};
use crate::metrics::Metrics;
use ipnet::IpNet;
use kube::api::{ListParams, PostParams};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;

/// `createdBy` prefix marking a block as machine-made.
pub const WORKFLOW_AUTHOR_PREFIX: &str = "waf-manager.inf-k8s.net/workflow/";

const DECISION_LOG_CAPACITY: usize = 200;

/// Everything a matcher can read about one candidate IP, fetched on demand.
#[derive(Debug, Default)]
pub struct IpFacts {
    pub client_ip: String,
    pub detections: u64,
    pub rules: Option<Vec<RuleHit>>,
    pub uris: Option<Vec<UriHit>>,
    /// Keyed `<workflow>/<signal>`, so two workflows may reuse a name.
    pub signals: BTreeMap<String, u64>,
}

impl IpFacts {
    pub fn new(client_ip: impl Into<String>, detections: u64) -> Self {
        Self {
            client_ip: client_ip.into(),
            detections,
            rules: None,
            uris: None,
            signals: BTreeMap::new(),
        }
    }

    fn rules(&self) -> &[RuleHit] {
        self.rules.as_deref().unwrap_or_default()
    }

    fn uris(&self) -> &[UriHit] {
        self.uris.as_deref().unwrap_or_default()
    }

    fn top_rule_ids(&self) -> Vec<String> {
        self.rules()
            .iter()
            .take(10)
            .map(|r| r.rule_id.clone())
            .collect()
    }
}

#[derive(Debug, Default, Clone)]
struct Needs {
    rules: bool,
    uris: bool,
    signals: BTreeSet<String>,
}

impl Needs {
    fn of(matcher: &Matcher) -> Self {
        match matcher {
            Matcher::Combinator(Combinator::All(ms)) | Matcher::Combinator(Combinator::Any(ms)) => {
                ms.iter()
                    .fold(Needs::default(), |acc, m| acc.merge(Needs::of(m)))
            }
            Matcher::Combinator(Combinator::Not(m)) => Needs::of(m),
            Matcher::Leaf(leaf) => match leaf {
                LeafMatcher::Detections { .. } => Needs::default(),
                LeafMatcher::DistinctRules { .. }
                | LeafMatcher::RuleId { .. }
                | LeafMatcher::RuleMsgMatches { .. }
                | LeafMatcher::Severity { .. } => Needs {
                    rules: true,
                    ..Needs::default()
                },
                LeafMatcher::UriMatches { .. } => Needs {
                    uris: true,
                    ..Needs::default()
                },
                LeafMatcher::Signal { name, .. } => Needs {
                    signals: [name.clone()].into_iter().collect(),
                    ..Needs::default()
                },
            },
        }
    }

    fn merge(mut self, other: Needs) -> Self {
        self.rules |= other.rules;
        self.uris |= other.uris;
        self.signals.extend(other.signals);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CandidateSource {
    window: Span,
    /// `None` means the built-in "top detecting client IPs" query.
    query: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Decision {
    pub at: chrono::DateTime<chrono::Utc>,
    pub workflow: String,
    pub cidr: String,
    pub detections: u64,
    pub mode: &'static str,
    pub outcome: String,
}

/// Evaluates the workflows in `config.yaml` against Loki and creates WafBlocks
/// for the IPs that match.
pub struct WorkflowEngine {
    config: Config,
    loki: Arc<Loki>,
    ctx: Arc<Context>,
    suppressions: crate::suppression::Suppressions,
    decisions: tokio::sync::RwLock<VecDeque<Decision>>,
}

impl WorkflowEngine {
    pub fn new(
        config: Config,
        loki: Arc<Loki>,
        ctx: Arc<Context>,
        suppressions: crate::suppression::Suppressions,
    ) -> Self {
        Self {
            config,
            loki,
            ctx,
            suppressions,
            decisions: tokio::sync::RwLock::new(VecDeque::with_capacity(DECISION_LOG_CAPACITY)),
        }
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    pub async fn suppress(&self, net: &IpNet) -> Result<()> {
        self.suppressions.record(net, chrono::Utc::now()).await
    }

    pub async fn decisions(&self) -> Vec<Decision> {
        let log = self.decisions.read().await;
        log.iter().rev().cloned().collect()
    }

    /// A source failing is logged and skipped, so one bad query cannot stall
    /// the rest.
    pub async fn run_once(&self) -> Result<()> {
        let now = chrono::Utc::now();
        let started = std::time::Instant::now();

        self.report_workflow_counts();

        if let Err(e) = self.suppressions.prune(now).await {
            tracing::warn!("pruning suppressions failed: {e}");
        }

        let suppressed = self.suppressions.active(now).await.unwrap_or_else(|e| {
            tracing::warn!("reading suppressions failed: {e}");
            Vec::new()
        });

        Metrics::set_suppressions(suppressed.len());

        let blocked = self.report_blocks().await?;

        for (source, workflows) in self.candidate_sources() {
            if let Err(e) = self
                .run_source(&source, &workflows, &blocked, &suppressed, now)
                .await
            {
                Metrics::record_loki_error();
                tracing::warn!(window = %source.window, "workflow candidate source failed: {e}");
            }
        }

        Metrics::record_workflow_run(started.elapsed());
        Ok(())
    }

    fn report_workflow_counts(&self) {
        for mode in [Enabled::Active, Enabled::DryRun, Enabled::Disabled] {
            let count = self
                .config
                .workflows
                .iter()
                .filter(|w| w.enabled == mode)
                .count();
            Metrics::set_workflows(mode.as_str(), count);
        }
    }

    /// Returns the blocklist too, so it is fetched once per run.
    async fn report_blocks(&self) -> Result<Vec<IpNet>> {
        let blocks = self.ctx.blocks().list(&ListParams::default()).await?.items;

        let automatic = blocks
            .iter()
            .filter(|b| {
                b.spec
                    .created_by
                    .as_deref()
                    .is_some_and(|by| by.starts_with(WORKFLOW_AUTHOR_PREFIX))
            })
            .count();

        Metrics::set_blocks_by_origin("workflow", automatic);
        Metrics::set_blocks_by_origin("manual", blocks.len() - automatic);

        Ok(blocks
            .iter()
            .filter_map(|b| b.spec.cidr.parse::<IpNet>().ok())
            .collect())
    }

    async fn run_source(
        &self,
        source: &CandidateSource,
        workflows: &[&WorkflowDef],
        blocked: &[IpNet],
        suppressed: &[IpNet],
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<()> {
        let window = source.window;
        let candidates = match &source.query {
            Some(query) => self.loki.candidates_from(query).await?,
            None => {
                self.loki
                    .top_client_ips(&window.to_logql(), self.config.defaults.candidate_limit)
                    .await?
            }
        };

        for candidate in candidates {
            let Ok(net) = self
                .ctx
                .allowlist
                .parse_and_check(&candidate.client_ip)
                .await
            else {
                // The normal case for a Cloudflare-fronted gateway, not an alert.
                continue;
            };

            if blocked.iter().any(|b| b.contains(&net)) {
                continue;
            }

            if suppressed.iter().any(|s| s.contains(&net)) {
                Metrics::record_workflow_skipped("suppressed");
                continue;
            }

            let mut facts = IpFacts::new(&candidate.client_ip, candidate.detections);

            for workflow in workflows {
                Metrics::record_workflow_evaluation(&workflow.name);
                self.fill_facts(&mut facts, workflow, window).await?;

                if !Self::evaluate(
                    &workflow.name,
                    &workflow.matcher,
                    &facts,
                    &self.config.ignored_rule_ids,
                ) {
                    continue;
                }

                self.apply(workflow, &net, &facts, now).await;
                // One block per IP per tick.
                break;
            }
        }

        Ok(())
    }

    async fn fill_facts(
        &self,
        facts: &mut IpFacts,
        workflow: &WorkflowDef,
        window: Span,
    ) -> Result<()> {
        let needs = Needs::of(&workflow.matcher);
        let window = window.to_logql();

        if needs.rules && facts.rules.is_none() {
            facts.rules = Some(self.loki.rules_for_ip(&facts.client_ip, &window).await?);
        }

        if needs.uris && facts.uris.is_none() {
            facts.uris = Some(self.loki.uris_for_ip(&facts.client_ip, &window).await?);
        }

        for name in &needs.signals {
            let key = Self::signal_key(&workflow.name, name);
            if facts.signals.contains_key(&key) {
                continue;
            }

            let Some(template) = workflow.signals.get(name) else {
                // build.rs rejects this; reaching it means a hand-written ConfigMap.
                tracing::warn!(
                    workflow = workflow.name,
                    signal = name,
                    "matcher references an undeclared signal; treating as 0"
                );
                facts.signals.insert(key, 0);
                continue;
            };

            let vars = Self::signal_vars(
                &window,
                &facts.client_ip,
                self.config.defaults.candidate_limit,
            );
            let missing = template.unresolved(&vars);
            if !missing.is_empty() {
                tracing::warn!(
                    workflow = workflow.name,
                    signal = name,
                    "signal has unresolved placeholders {missing:?}; treating as 0"
                );
                facts.signals.insert(key, 0);
                continue;
            }

            let value = self.loki.scalar(&template.render(&vars)).await?;
            facts.signals.insert(key, value);
        }

        Ok(())
    }

    fn signal_key(workflow: &str, signal: &str) -> String {
        format!("{workflow}/{signal}")
    }

    fn signal_vars<'a>(window: &str, client_ip: &str, limit: usize) -> BTreeMap<&'a str, String> {
        let (prefilter, exact_client, sanitised) = Loki::client_filters(client_ip);
        let mut vars = Self::base_vars(window, limit);

        vars.insert("client_ip", sanitised);
        vars.insert("prefilter", prefilter);
        vars.insert("exact_client", exact_client);
        vars
    }

    fn base_vars<'a>(window: &str, limit: usize) -> BTreeMap<&'a str, String> {
        let mut vars: BTreeMap<&str, String> = Loki::fragments()
            .into_iter()
            .map(|(k, v)| (k, v.to_string()))
            .collect();

        vars.insert("window", window.to_string());
        vars.insert("limit", limit.to_string());
        vars
    }

    async fn apply(
        &self,
        workflow: &WorkflowDef,
        net: &IpNet,
        facts: &IpFacts,
        now: chrono::DateTime<chrono::Utc>,
    ) {
        let mode = workflow.enabled.as_str();

        if workflow.enabled == Enabled::DryRun {
            tracing::info!(
                workflow = workflow.name,
                cidr = %net,
                detections = facts.detections,
                "[dry-run] would block"
            );

            Metrics::record_workflow_block(&workflow.name, mode);
            self.record(Decision {
                at: now,
                workflow: workflow.name.clone(),
                cidr: net.to_string(),
                detections: facts.detections,
                mode,
                outcome: "would block".to_string(),
            })
            .await;

            return;
        }

        let spec = WafBlockSpec {
            cidr: net.to_string(),
            gateway: workflow.gateway(&self.config.defaults).to_string(),
            reason: Some(workflow.reason.clone()),
            rule_ids: Some(facts.top_rule_ids()).filter(|ids| !ids.is_empty()),
            expires_at: workflow.expires_at(now),
            created_by: Some(format!("{WORKFLOW_AUTHOR_PREFIX}{}", workflow.name)),
        };

        let block = WafBlock::new(&WafBlock::resource_name(net), spec);
        let outcome = match self
            .ctx
            .blocks()
            .create(&PostParams::default(), &block)
            .await
        {
            Ok(_) => {
                tracing::info!(
                    workflow = workflow.name,
                    cidr = %net,
                    detections = facts.detections,
                    "blocked"
                );

                Metrics::record_workflow_block(&workflow.name, mode);
                "blocked".to_string()
            }
            Err(e) => {
                tracing::warn!(workflow = workflow.name, cidr = %net, "block failed: {e}");
                Metrics::record_workflow_error(&workflow.name);
                format!("failed: {e}")
            }
        };

        self.record(Decision {
            at: now,
            workflow: workflow.name.clone(),
            cidr: net.to_string(),
            detections: facts.detections,
            mode,
            outcome,
        })
        .await;
    }

    async fn record(&self, decision: Decision) {
        let mut log = self.decisions.write().await;
        if log.len() >= DECISION_LOG_CAPACITY {
            log.pop_front();
        }

        log.push_back(decision);
    }

    /// Workflows sharing a window and source cost one query between them.
    fn candidate_sources(&self) -> Vec<(CandidateSource, Vec<&WorkflowDef>)> {
        let mut grouped: Vec<(CandidateSource, Vec<&WorkflowDef>)> = Vec::new();

        for workflow in self.config.active_workflows() {
            let window = workflow.window(&self.config.defaults);
            let vars = Self::base_vars(&window.to_logql(), self.config.defaults.candidate_limit);

            let query = workflow.candidates.as_ref().and_then(|template| {
                let missing = template.unresolved(&vars);
                if missing.is_empty() {
                    return Some(template.render(&vars));
                }

                tracing::warn!(
                    workflow = workflow.name,
                    "candidates query has unresolved placeholders {missing:?}; using the default"
                );
                None
            });

            let source = CandidateSource { window, query };
            match grouped.iter_mut().find(|(s, _)| *s == source) {
                Some((_, workflows)) => workflows.push(workflow),
                None => grouped.push((source, vec![workflow])),
            }
        }

        grouped
    }

    /// `workflow` names the signal namespace.
    pub fn evaluate(
        workflow: &str,
        matcher: &Matcher,
        facts: &IpFacts,
        ignored: &BTreeSet<String>,
    ) -> bool {
        match matcher {
            Matcher::Combinator(Combinator::All(ms)) => ms
                .iter()
                .all(|m| Self::evaluate(workflow, m, facts, ignored)),
            Matcher::Combinator(Combinator::Any(ms)) => ms
                .iter()
                .any(|m| Self::evaluate(workflow, m, facts, ignored)),
            Matcher::Combinator(Combinator::Not(m)) => !Self::evaluate(workflow, m, facts, ignored),
            Matcher::Leaf(leaf) => Self::evaluate_leaf(workflow, leaf, facts, ignored),
        }
    }

    fn evaluate_leaf(
        workflow: &str,
        leaf: &LeafMatcher,
        facts: &IpFacts,
        ignored: &BTreeSet<String>,
    ) -> bool {
        let interesting = || {
            facts
                .rules()
                .iter()
                .filter(|r| !ignored.contains(&r.rule_id))
        };

        match leaf {
            LeafMatcher::Detections { min, max } => {
                facts.detections >= *min && max.is_none_or(|max| facts.detections <= max)
            }

            LeafMatcher::DistinctRules { min } => {
                let distinct: BTreeSet<&str> = interesting().map(|r| r.rule_id.as_str()).collect();
                distinct.len() >= *min
            }

            LeafMatcher::RuleId { values, min_count } => {
                let hit: u64 = interesting()
                    .filter(|r| values.contains(&r.rule_id))
                    .map(|r| r.count)
                    .sum();
                hit >= *min_count
            }

            LeafMatcher::RuleMsgMatches {
                patterns,
                mode,
                min_count,
            } => {
                let hit: u64 = interesting()
                    .filter(|r| patterns.iter().any(|p| mode.matches(&r.rule_msg, p)))
                    .map(|r| r.count)
                    .sum();
                hit >= *min_count
            }

            LeafMatcher::Severity { values, min_count } => {
                let hit: u64 = interesting()
                    .filter(|r| values.iter().any(|v| v.eq_ignore_ascii_case(&r.severity)))
                    .map(|r| r.count)
                    .sum();
                hit >= *min_count
            }

            LeafMatcher::UriMatches {
                patterns,
                mode,
                min_count,
            } => {
                let hit: u64 = facts
                    .uris()
                    .iter()
                    .filter(|u| patterns.iter().any(|p| mode.matches(&u.uri, p)))
                    .map(|u| u.count)
                    .sum();
                hit >= *min_count
            }

            LeafMatcher::Signal { name, min, max } => {
                // Absent rather than zero, so it cannot satisfy `min: 0`.
                let Some(value) = facts.signals.get(&Self::signal_key(workflow, name)) else {
                    return false;
                };

                *value >= *min && max.is_none_or(|max| *value <= max)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matcher(yaml: &str) -> Matcher {
        serde_yaml::from_str(yaml).expect(yaml)
    }

    fn rule(id: &str, msg: &str, severity: &str, count: u64) -> RuleHit {
        RuleHit {
            rule_id: id.to_string(),
            rule_msg: msg.to_string(),
            severity: severity.to_string(),
            count,
        }
    }

    fn facts() -> IpFacts {
        IpFacts {
            client_ip: "203.0.113.4".to_string(),
            detections: 640,
            rules: Some(vec![
                rule("930120", "OS File Access Attempt", "critical", 400),
                rule("930130", "Restricted File Access Attempt", "critical", 200),
                rule("949110", "Inbound Anomaly Score Exceeded", "error", 40),
            ]),
            uris: Some(vec![
                UriHit {
                    uri: "/.env".to_string(),
                    count: 300,
                },
                UriHit {
                    uri: "/index.html".to_string(),
                    count: 12,
                },
            ]),
            signals: [("test/sqli".to_string(), 42)].into_iter().collect(),
        }
    }

    fn ignored() -> BTreeSet<String> {
        ["949110".to_string()].into_iter().collect()
    }

    fn eval(yaml: &str) -> bool {
        WorkflowEngine::evaluate("test", &matcher(yaml), &facts(), &ignored())
    }

    #[test]
    fn detection_volume_compares_both_bounds() {
        assert!(eval("type: detections\nmin: 500"));
        assert!(!eval("type: detections\nmin: 1000"));
        assert!(!eval("type: detections\nmin: 100\nmax: 500"));
    }

    #[test]
    fn ignored_rules_do_not_count_towards_distinct_rules() {
        // Three hits, but 949110 is noise, so only two are findings.
        assert!(eval("type: distinct_rules\nmin: 2"));
        assert!(!eval("type: distinct_rules\nmin: 3"));
    }

    #[test]
    fn an_ignored_rule_id_never_matches_on_its_own() {
        assert!(!eval("type: rule_id\nvalues: [\"949110\"]"));
        assert!(eval("type: rule_id\nvalues: [\"930120\"]"));
    }

    #[test]
    fn rule_id_counts_are_summed_against_min_count() {
        assert!(eval(
            "type: rule_id\nvalues: [\"930120\", \"930130\"]\nmin_count: 600"
        ));
        assert!(!eval(
            "type: rule_id\nvalues: [\"930120\", \"930130\"]\nmin_count: 601"
        ));
    }

    #[test]
    fn env_probes_match_on_uri() {
        assert!(eval("type: uri_matches\npatterns: [\"/.env\"]"));
        assert!(!eval("type: uri_matches\npatterns: [\"/wp-admin\"]"));
    }

    #[test]
    fn uri_min_count_gates_a_single_stray_request() {
        assert!(!eval(
            "type: uri_matches\npatterns: [\"/index.html\"]\nmin_count: 100"
        ));
    }

    #[test]
    fn severity_matching_ignores_case() {
        assert!(eval("type: severity\nvalues: [CRITICAL]\nmin_count: 600"));
        assert!(!eval("type: severity\nvalues: [notice]"));
    }

    #[test]
    fn rule_messages_match_by_substring() {
        assert!(eval(
            "type: rule_msg_matches\npatterns: [\"Restricted File\"]"
        ));
        assert!(!eval(
            "type: rule_msg_matches\npatterns: [\"SQL Injection\"]"
        ));
    }

    #[test]
    fn combinators_nest() {
        assert!(eval(
            "all:\n  - type: detections\n    min: 500\n  - any:\n      - type: uri_matches\n        patterns: [\"/.env\"]\n      - type: distinct_rules\n        min: 99"
        ));

        assert!(!eval(
            "all:\n  - type: detections\n    min: 500\n  - not:\n      type: uri_matches\n      patterns: [\"/.env\"]"
        ));
    }

    #[test]
    fn absent_facts_never_match() {
        let bare = IpFacts::new("203.0.113.4", 10);
        let ignored = BTreeSet::new();

        assert!(!WorkflowEngine::evaluate(
            "test",
            &matcher("type: uri_matches\npatterns: [\"/.env\"]"),
            &bare,
            &ignored
        ));
        assert!(!WorkflowEngine::evaluate(
            "test",
            &matcher("type: distinct_rules\nmin: 1"),
            &bare,
            &ignored
        ));
    }

    #[test]
    fn only_the_facts_a_workflow_reads_are_fetched() {
        let volume_only = Needs::of(&matcher("type: detections\nmin: 5"));
        assert!(!volume_only.rules && !volume_only.uris);

        let both = Needs::of(&matcher(
            "all:\n  - type: uri_matches\n    patterns: [\"/.env\"]\n  - type: severity\n    values: [critical]",
        ));
        assert!(both.rules && both.uris);
    }

    #[test]
    fn signals_compare_against_their_bounds() {
        assert!(eval("type: signal\nname: sqli\nmin: 42"));
        assert!(!eval("type: signal\nname: sqli\nmin: 43"));
        assert!(!eval("type: signal\nname: sqli\nmin: 1\nmax: 41"));
    }

    #[test]
    fn an_unloaded_signal_never_matches() {
        // `min: 0` would otherwise be satisfied by a query that never ran.
        assert!(!eval("type: signal\nname: never_fetched\nmin: 0"));
    }

    #[test]
    fn signals_are_namespaced_by_workflow() {
        assert!(!WorkflowEngine::evaluate(
            "other",
            &matcher("type: signal\nname: sqli\nmin: 42"),
            &facts(),
            &ignored()
        ));
    }

    #[test]
    fn recorded_rule_ids_are_capped() {
        let mut facts = facts();
        facts.rules = Some(
            (0..25)
                .map(|i| rule(&i.to_string(), "m", "error", 1))
                .collect(),
        );

        assert_eq!(facts.top_rule_ids().len(), 10);
    }
}
