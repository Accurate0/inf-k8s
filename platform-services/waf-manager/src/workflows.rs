use crate::config::{
    CandidateStrategy, Combinator, Config, Enabled, LeafMatcher, Matcher, Span, Tier, WorkflowDef,
};
use crate::controller::Context;
use crate::crd::{WafBlock, WafBlockSpec};
use crate::error::Result;
use crate::loki::{Loki, RuleHit, UriHit};
use crate::metrics::Metrics;
use ipnet::IpNet;
use kube::api::{ListParams, PostParams};
use sqlx::postgres::PgPool;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

pub const WORKFLOW_AUTHOR_PREFIX: &str = "waf-manager.inf-k8s.net/workflow/";

const DECISION_PAGE_SIZE: i64 = 200;

const SCORECARD_DAYS: i64 = 30;

pub const MANUAL_MODE: &str = "manual";
pub const UNBLOCKED_OUTCOME: &str = "unblocked";

#[derive(Debug, Clone, Default)]
pub struct Scorecard {
    pub workflow: String,
    pub enabled: &'static str,
    pub decisions: i64,
    pub cidrs: i64,
    pub unblocked: i64,
    pub agreement: i64,
}

impl Scorecard {
    pub fn verdict(&self) -> &'static str {
        if self.decisions == 0 {
            return "no data";
        }

        if self.unblocked > 0 && self.unblocked * 4 >= self.cidrs {
            return "disputed";
        }

        if self.agreement * 2 >= self.cidrs {
            return "agrees";
        }

        "unproven"
    }
}

#[derive(Debug, Default)]
pub struct IpFacts {
    pub client_ip: String,
    pub detections: u64,
    pub rules: Option<Vec<RuleHit>>,
    pub uris: Option<Vec<UriHit>>,
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
    query: Option<String>,
    strategy: CandidateStrategy,
    limit: usize,
}

#[derive(Debug, Clone)]
pub struct Decision {
    pub at: chrono::DateTime<chrono::Utc>,
    pub workflow: String,
    pub cidr: String,
    pub detections: u64,
    pub mode: String,
    pub outcome: String,
}

impl Decision {
    pub fn host(&self) -> Option<&str> {
        self.cidr
            .strip_suffix("/32")
            .or_else(|| self.cidr.strip_suffix("/128"))
    }
}

#[derive(Clone)]
pub struct WorkflowEngine {
    inner: Arc<WorkflowEngineInner>,
}

struct WorkflowEngineInner {
    config: Config,
    loki: Arc<Loki>,
    ctx: Arc<Context>,
    suppressions: crate::suppression::Suppressions,
    offenses: crate::offenses::Offenses,
    pool: PgPool,
}

impl WorkflowEngine {
    pub fn new(
        config: Config,
        loki: Arc<Loki>,
        ctx: Arc<Context>,
        suppressions: crate::suppression::Suppressions,
        pool: PgPool,
    ) -> Self {
        let offenses =
            crate::offenses::Offenses::new(pool.clone(), config.defaults.escalation_decay);

        Self {
            inner: Arc::new(WorkflowEngineInner {
                config,
                loki,
                ctx,
                suppressions,
                offenses,
                pool,
            }),
        }
    }

    pub fn config(&self) -> &Config {
        &self.inner.config
    }

    pub async fn import_suppressions(&self) -> Result<()> {
        self.inner
            .suppressions
            .import_configmap(chrono::Utc::now())
            .await
    }

    pub async fn suppress(&self, net: &IpNet) -> Result<()> {
        self.inner
            .suppressions
            .record(net, chrono::Utc::now())
            .await
    }

    pub async fn forgive(&self, net: &IpNet) -> Result<()> {
        self.inner.offenses.reset(net).await
    }

    pub async fn strikes(&self, net: &IpNet) -> Result<i32> {
        self.inner.offenses.strikes(net).await
    }

    pub async fn all_strikes(&self) -> Result<std::collections::BTreeMap<String, i32>> {
        self.inner.offenses.all().await
    }

    pub async fn decisions(&self, workflow: Option<&str>) -> Result<Vec<Decision>> {
        let rows = sqlx::query!(
            "select at, workflow, cidr, detections, mode, outcome
             from decisions where ($2::text is null or workflow = $2)
             order by at desc, id desc limit $1",
            DECISION_PAGE_SIZE,
            workflow,
        )
        .fetch_all(&self.inner.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| Decision {
                at: row.at,
                workflow: row.workflow,
                cidr: row.cidr,
                detections: row.detections as u64,
                mode: row.mode,
                outcome: row.outcome,
            })
            .collect())
    }

    pub async fn decisions_for(&self, cidr: &str) -> Result<Vec<Decision>> {
        let rows = sqlx::query!(
            "select at, workflow, cidr, detections, mode, outcome
             from decisions where cidr = $1
             order by at desc, id desc limit $2",
            cidr,
            DECISION_PAGE_SIZE,
        )
        .fetch_all(&self.inner.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| Decision {
                at: row.at,
                workflow: row.workflow,
                cidr: row.cidr,
                detections: row.detections as u64,
                mode: row.mode,
                outcome: row.outcome,
            })
            .collect())
    }

    pub async fn record_unblock(&self, workflow: &str, net: &IpNet) {
        Metrics::record_workflow_unblock(workflow);

        self.record(Decision {
            at: chrono::Utc::now(),
            workflow: workflow.to_string(),
            cidr: net.to_string(),
            detections: 0,
            mode: MANUAL_MODE.to_string(),
            outcome: UNBLOCKED_OUTCOME.to_string(),
        })
        .await;
    }

    pub async fn scorecard(&self) -> Result<Vec<Scorecard>> {
        let active: BTreeSet<&str> = self
            .config()
            .workflows
            .iter()
            .filter(|w| w.enabled == Enabled::Active)
            .map(|w| w.name.as_str())
            .collect();

        let since = chrono::Utc::now() - chrono::Duration::days(SCORECARD_DAYS);
        let active_names: Vec<String> = active.iter().map(|n| n.to_string()).collect();

        let rows = sqlx::query!(
            "with recent as (
                 select workflow, cidr, outcome from decisions where at >= $1
             ),
             agreed as (
                 select distinct workflow, cidr from recent
                 where workflow = any($2::text[]) and outcome <> $3
             )
             select
                 r.workflow as workflow,
                 count(*) as decisions,
                 count(distinct r.cidr) as cidrs,
                 count(*) filter (where r.outcome = $3) as unblocked,
                 count(distinct r.cidr) filter (
                     where exists (
                         select 1 from agreed a
                         where a.cidr = r.cidr and a.workflow <> r.workflow
                     )
                 ) as agreement
             from recent r
             group by r.workflow",
            since,
            &active_names,
            UNBLOCKED_OUTCOME,
        )
        .fetch_all(&self.inner.pool)
        .await?;

        let mut by_workflow: BTreeMap<String, Scorecard> = rows
            .into_iter()
            .map(|row| {
                let card = Scorecard {
                    workflow: row.workflow.clone(),
                    enabled: "",
                    decisions: row.decisions.unwrap_or(0),
                    cidrs: row.cidrs.unwrap_or(0),
                    unblocked: row.unblocked.unwrap_or(0),
                    agreement: row.agreement.unwrap_or(0),
                };

                (row.workflow, card)
            })
            .collect();

        Ok(self
            .config()
            .workflows
            .iter()
            .map(|w| {
                let mut card = by_workflow.remove(&w.name).unwrap_or(Scorecard {
                    workflow: w.name.clone(),
                    ..Default::default()
                });

                card.enabled = w.enabled.as_str();
                card
            })
            .collect())
    }

    pub async fn run_once(&self) -> Result<()> {
        let now = chrono::Utc::now();
        let started = std::time::Instant::now();

        self.report_workflow_counts();

        if !self.inner.ctx.allowlist.ready() {
            tracing::warn!("allowlist has not loaded yet, skipping this workflow run");
            Metrics::record_workflow_skipped("allowlist-cold");
            return Ok(());
        }

        if let Err(e) = self.inner.suppressions.prune(now).await {
            tracing::warn!("pruning suppressions failed: {e}");
        }

        if let Err(e) = self.inner.offenses.prune(now).await {
            tracing::warn!("pruning offenses failed: {e}");
        }

        if let Err(e) = self.inner.offenses.report().await {
            tracing::warn!("counting offenses failed: {e}");
        }

        self.run_tier(Tier::Standard, now).await?;

        Metrics::record_workflow_run(started.elapsed());
        Ok(())
    }

    pub async fn run_fast(&self) -> Result<()> {
        let now = chrono::Utc::now();
        let started = std::time::Instant::now();

        if !self.inner.ctx.allowlist.ready() {
            Metrics::record_workflow_skipped("allowlist-cold");
            return Ok(());
        }

        self.run_tier(Tier::Fast, now).await?;

        Metrics::record_workflow_run(started.elapsed());
        Ok(())
    }

    async fn run_tier(&self, tier: Tier, now: chrono::DateTime<chrono::Utc>) -> Result<()> {
        let sources = self.candidate_sources(tier);

        if sources.is_empty() {
            return Ok(());
        }

        let suppressed = self
            .inner
            .suppressions
            .active(now)
            .await
            .unwrap_or_else(|e| {
                tracing::warn!("reading suppressions failed: {e}");
                Vec::new()
            });

        if tier == Tier::Standard {
            Metrics::set_suppressions(suppressed.len());
        }

        let blocked = self.report_blocks().await?;

        for (source, workflows) in sources {
            if let Err(e) = self
                .run_source(&source, &workflows, &blocked, &suppressed, now)
                .await
            {
                Metrics::record_loki_error();
                tracing::warn!(window = %source.window, "workflow candidate source failed: {e}");
            }
        }

        Ok(())
    }

    fn report_workflow_counts(&self) {
        for mode in [Enabled::Active, Enabled::DryRun, Enabled::Disabled] {
            let count = self
                .config()
                .workflows
                .iter()
                .filter(|w| w.enabled == mode)
                .count();
            Metrics::set_workflows(mode.as_str(), count);
        }
    }

    async fn report_blocks(&self) -> Result<Vec<IpNet>> {
        let blocks = self
            .inner
            .ctx
            .blocks()
            .list(&ListParams::default())
            .await?
            .items;

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
        let candidates = match (&source.query, source.strategy) {
            (Some(query), _) => self.inner.loki.candidates_from(query).await?,
            (None, CandidateStrategy::Detections) => {
                self.inner
                    .loki
                    .top_client_ips(&window.to_logql(), source.limit)
                    .await?
            }
            (None, CandidateStrategy::DistinctRules) => {
                self.inner
                    .loki
                    .top_client_ips_by_distinct_rules(&window.to_logql(), source.limit)
                    .await?
            }
        };

        let total = candidates.len();
        let mut failed = 0usize;

        for candidate in candidates {
            let Ok(net) = self
                .inner
                .ctx
                .allowlist
                .parse_and_check(&candidate.client_ip)
                .await
            else {
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

                if let Err(e) = self.fill_facts(&mut facts, workflow, window).await {
                    tracing::debug!(
                        workflow = workflow.name,
                        cidr = %net,
                        "gathering facts failed, skipping this candidate: {e}"
                    );

                    Metrics::record_workflow_error(&workflow.name);
                    Metrics::record_loki_error();
                    failed += 1;
                    break;
                }

                if !Self::evaluate(
                    &workflow.name,
                    &workflow.matcher,
                    &facts,
                    &self.inner.config.ignored_rule_ids,
                ) {
                    continue;
                }

                self.apply(workflow, &net, &facts, now).await;
                break;
            }
        }

        if failed > 0 {
            tracing::warn!(
                window = %window,
                "gathering facts failed for {failed} of {total} candidates"
            );
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
            facts.rules = Some(
                self.inner
                    .loki
                    .rules_for_ip(&facts.client_ip, &window)
                    .await?,
            );
        }

        if needs.uris && facts.uris.is_none() {
            facts.uris = Some(
                self.inner
                    .loki
                    .uris_for_ip(&facts.client_ip, &window)
                    .await?,
            );
        }

        for name in &needs.signals {
            let key = Self::signal_key(&workflow.name, name);
            if facts.signals.contains_key(&key) {
                continue;
            }

            let Some(template) = workflow.signals.get(name) else {
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
                self.inner.config.defaults.candidate_limit,
            )?;
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

            let value = self.inner.loki.scalar(&template.render(&vars)).await?;
            facts.signals.insert(key, value);
        }

        Ok(())
    }

    fn signal_key(workflow: &str, signal: &str) -> String {
        format!("{workflow}/{signal}")
    }

    fn signal_vars<'a>(
        window: &str,
        client_ip: &str,
        limit: usize,
    ) -> Result<BTreeMap<&'a str, String>> {
        let (prefilter, exact_client, sanitised) = Loki::client_filters(client_ip)?;
        let mut vars = Self::base_vars(window, limit);

        vars.insert("client_ip", sanitised);
        vars.insert("prefilter", prefilter);
        vars.insert("exact_client", exact_client);
        Ok(vars)
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
                mode: mode.to_string(),
                outcome: "would block".to_string(),
            })
            .await;

            return;
        }

        let strikes = match self.inner.offenses.record(net, now).await {
            Ok(strikes) => strikes,
            Err(e) => {
                tracing::warn!(cidr = %net, "recording the offense failed, treating it as a first strike: {e}");
                1
            }
        };

        let spec = WafBlockSpec {
            cidr: net.to_string(),
            gateway: workflow.gateway(&self.inner.config.defaults).to_string(),
            reason: Some(workflow.reason.clone()),
            rule_ids: Some(facts.top_rule_ids()).filter(|ids| !ids.is_empty()),
            expires_at: workflow.expires_at(now, strikes, &self.inner.config.defaults),
            created_by: Some(format!("{WORKFLOW_AUTHOR_PREFIX}{}", workflow.name)),
        };

        let block = WafBlock::new(&WafBlock::resource_name(net), spec);
        let outcome = match self
            .inner
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

                if workflow.tier == Tier::Fast {
                    self.inner.ctx.request_sync();
                }

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
            mode: mode.to_string(),
            outcome,
        })
        .await;
    }

    async fn record(&self, decision: Decision) {
        let result = sqlx::query!(
            "insert into decisions (at, workflow, cidr, detections, mode, outcome)
             values ($1, $2, $3, $4, $5, $6)",
            decision.at,
            decision.workflow,
            decision.cidr,
            decision.detections as i64,
            decision.mode,
            decision.outcome,
        )
        .execute(&self.inner.pool)
        .await;

        if let Err(e) = result {
            tracing::warn!("recording decision for {} failed: {e}", decision.cidr);
        }
    }

    fn candidate_sources(&self, tier: Tier) -> Vec<(CandidateSource, Vec<&WorkflowDef>)> {
        Self::sources_for(&self.inner.config, tier)
    }

    fn sources_for(config: &Config, tier: Tier) -> Vec<(CandidateSource, Vec<&WorkflowDef>)> {
        let defaults = &config.defaults;
        let limit = match tier {
            Tier::Fast => defaults.fast_candidate_limit,
            Tier::Standard => defaults.candidate_limit,
        };

        let mut grouped: Vec<(CandidateSource, Vec<&WorkflowDef>)> = Vec::new();

        for workflow in config.active_workflows().filter(|w| w.tier == tier) {
            let window = workflow.window(defaults);
            let vars = Self::base_vars(&window.to_logql(), limit);

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

            let source = CandidateSource {
                window,
                query,
                strategy: workflow.candidate_strategy,
                limit,
            };
            match grouped.iter_mut().find(|(s, _)| *s == source) {
                Some((_, workflows)) => workflows.push(workflow),
                None => grouped.push((source, vec![workflow])),
            }
        }

        grouped
    }

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

    fn config(workflows: &str) -> Config {
        let yaml = format!(
            "version: 2\ndefaults:\n  window: 12h\n  candidate_limit: 50\nworkflows:\n{workflows}"
        );

        serde_yaml::from_str(&yaml).expect(&yaml)
    }

    fn workflow(name: &str, strategy: &str) -> String {
        format!(
            "  - name: {name}\n    enabled: active\n    candidate_strategy: {strategy}\n    \
             duration: 24h\n    reason: test\n    when:\n      type: detections\n      min: 1\n"
        )
    }

    #[test]
    fn workflows_sharing_a_window_and_strategy_share_one_candidate_source() {
        let config = config(&format!(
            "{}{}",
            workflow("first", "detections"),
            workflow("second", "detections")
        ));

        let sources = WorkflowEngine::sources_for(&config, Tier::Standard);

        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].1.len(), 2);
    }

    #[test]
    fn a_different_strategy_gets_its_own_candidate_source() {
        let config = config(&format!(
            "{}{}",
            workflow("volume", "detections"),
            workflow("diversity", "distinct_rules")
        ));

        let sources = WorkflowEngine::sources_for(&config, Tier::Standard);

        assert_eq!(sources.len(), 2);
        assert!(
            sources
                .iter()
                .any(|(source, _)| source.strategy == CandidateStrategy::DistinctRules)
        );
        assert!(sources.iter().all(|(_, workflows)| workflows.len() == 1));
    }

    #[test]
    fn the_default_strategy_ranks_by_detection_volume() {
        let config = config(
            "  - name: plain\n    enabled: active\n    duration: 24h\n    \
                             reason: test\n    when:\n      type: detections\n      min: 1\n",
        );

        let sources = WorkflowEngine::sources_for(&config, Tier::Standard);

        assert_eq!(sources[0].0.strategy, CandidateStrategy::Detections);
    }

    #[test]
    fn a_workflow_with_no_decisions_has_no_verdict() {
        let card = Scorecard::default();

        assert_eq!(card.verdict(), "no data");
    }

    #[test]
    fn manual_unblocks_dispute_a_workflow() {
        let card = Scorecard {
            decisions: 20,
            cidrs: 8,
            unblocked: 2,
            agreement: 8,
            ..Scorecard::default()
        };

        assert_eq!(card.verdict(), "disputed");
    }

    #[test]
    fn agreement_with_an_active_workflow_reads_as_safe_to_promote() {
        let card = Scorecard {
            decisions: 20,
            cidrs: 10,
            unblocked: 0,
            agreement: 7,
            ..Scorecard::default()
        };

        assert_eq!(card.verdict(), "agrees");
    }

    #[test]
    fn decisions_nobody_else_saw_stay_unproven() {
        let card = Scorecard {
            decisions: 20,
            cidrs: 10,
            unblocked: 0,
            agreement: 1,
            ..Scorecard::default()
        };

        assert_eq!(card.verdict(), "unproven");
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
