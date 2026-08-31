use askama::Template;
use axum::extract::{Path, Query, Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::{Next, from_fn_with_state};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Extension, Form, Router};
use ipnet::IpNet;
use kube::ResourceExt;
use kube::api::{DeleteParams, ListParams, PostParams};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use tokio::sync::watch;
use waf_manager::audit;
use waf_manager::compositor::Conflict;
use waf_manager::controller::Context;
use waf_manager::crd::{WafBlock, WafBlockSpec};
use waf_manager::metrics::Metrics;
use waf_manager::workflows::{Decision, Scorecard, WORKFLOW_AUTHOR_PREFIX};
use waf_manager::{
    Allowlist, Audit, Claims, Enricher, Enrichment, Error, Jwks, Loki, ManualAllowlist,
    ManualEntry, WorkflowEngine,
};

const BLOCK_PAGE_SIZE: usize = 100;
const AUDIT_PAGE_SIZE: i64 = 100;
const IP_HISTORY_LIMIT: i64 = 20;

pub struct AppState {
    pub ctx: Arc<Context>,
    pub loki: Arc<Loki>,
    pub engine: Arc<WorkflowEngine>,
    pub leadership: watch::Receiver<bool>,
    pub jwks: Option<Jwks>,
    pub audit: Audit,
    pub manual: ManualAllowlist,
    pub enricher: Option<Enricher>,
}

impl AppState {
    fn window(&self) -> String {
        self.engine.config().defaults.window.to_logql()
    }

    fn window_label(&self) -> String {
        self.engine.config().defaults.window.to_string()
    }

    fn ignored_rules(&self) -> &BTreeSet<String> {
        &self.engine.config().ignored_rule_ids
    }
}

pub struct Routes;

impl Routes {
    pub fn router(state: Arc<AppState>) -> Router {
        let protected = Router::new()
            .route("/", get(Self::index))
            .route("/blocks", get(Self::blocks))
            .route("/workflows", get(Self::workflows))
            .route("/ip/{addr}", get(Self::ip_detail))
            .route("/audit", get(Self::audit))
            .route("/block", post(Self::create_block))
            .route("/unblock", post(Self::delete_block))
            .route("/allowlist", post(Self::create_allowlist))
            .route("/allowlist/delete", post(Self::delete_allowlist))
            .layer(from_fn_with_state(state.clone(), Self::authenticate))
            .with_state(state);

        let open = Router::new()
            .route("/health", get(|| async { StatusCode::OK }))
            .route("/metrics", get(|| async { Metrics::render() }));

        protected.merge(open)
    }

    async fn authenticate(
        State(state): State<Arc<AppState>>,
        mut request: Request,
        next: Next,
    ) -> Result<Response, AppError> {
        let claims = match &state.jwks {
            Some(jwks) => {
                let token = request
                    .headers()
                    .get(header::AUTHORIZATION)
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.strip_prefix("Bearer "))
                    .map(str::trim)
                    .filter(|t| !t.is_empty())
                    .ok_or_else(|| {
                        Metrics::record_auth_rejected("no_bearer_token");
                        Error::Unauthorized("no bearer token on request".to_string())
                    })?;

                jwks.verify(token).await.inspect_err(|_| {
                    Metrics::record_auth_rejected("invalid_token");
                })?
            }
            None => Claims {
                sub: "insecure-no-auth".to_string(),
                preferred_username: Some("insecure-no-auth".to_string()),
                email: None,
                name: None,
            },
        };

        request.extensions_mut().insert(claims);

        Ok(next.run(request).await)
    }

    async fn index(State(state): State<Arc<AppState>>) -> Result<Html<String>, AppError> {
        let limit = state.engine.config().defaults.candidate_limit;
        let candidates = match state.loki.top_client_ips(&state.window(), limit).await {
            Ok(c) => c,
            Err(e) => {
                Metrics::record_loki_error();
                return Err(e.into());
            }
        };

        let blocked = Self::blocked_cidrs(&state).await?;
        let protected = state.ctx.allowlist.entries().await;

        let mut actionable = Vec::new();
        let mut protected_rows = Vec::new();
        let mut blocked_rows = Vec::new();

        for c in candidates {
            let net = waf_manager::Allowlist::parse_cidr(&c.client_ip).ok();

            if net.is_some_and(|net| blocked.iter().any(|b| b.contains(&net))) {
                blocked_rows.push(CandidateRow::new(c.client_ip, c.detections));
                continue;
            }

            match net.and_then(|net| waf_manager::Allowlist::matching(&protected, &net)) {
                Some((range, source)) => protected_rows.push(ProtectedCandidateRow {
                    client_ip: c.client_ip,
                    detections: c.detections,
                    range: range.to_string(),
                    source: source.clone(),
                }),
                None => actionable.push(CandidateRow::new(c.client_ip, c.detections)),
            }
        }

        Self::render(IndexTemplate {
            actionable,
            protected: protected_rows,
            blocked: blocked_rows,
            window: state.window_label(),
        })
    }

    async fn blocks(
        State(state): State<Arc<AppState>>,
        Query(page): Query<BlocksQuery>,
    ) -> Result<Html<String>, AppError> {
        let mut blocks = state
            .ctx
            .blocks()
            .list(&ListParams::default())
            .await
            .map_err(Error::from)?
            .items;

        blocks.sort_by(|a, b| {
            b.creation_timestamp()
                .cmp(&a.creation_timestamp())
                .then_with(|| a.name_any().cmp(&b.name_any()))
        });

        let filter = page.q.trim().to_lowercase();

        if !filter.is_empty() {
            blocks.retain(|b| b.spec.cidr.to_lowercase().contains(&filter));
        }

        let total = blocks.len();
        let offset = page.offset.min(total);
        let blocks: Vec<_> = blocks
            .into_iter()
            .skip(offset)
            .take(BLOCK_PAGE_SIZE)
            .collect();

        let strikes = state.engine.all_strikes().await.unwrap_or_else(|e| {
            tracing::warn!("reading offenses failed: {e}");
            Default::default()
        });

        let rows: Vec<BlockRow> = blocks
            .into_iter()
            .map(|b| BlockRow::new(b, &strikes))
            .collect();

        let shown = offset + rows.len();

        Self::render(BlocksTemplate {
            rows,
            conflicts: state.ctx.conflicts().await?,
            total,
            offset,
            query: page.q.trim().to_string(),
            newer: (offset > 0).then(|| offset.saturating_sub(BLOCK_PAGE_SIZE)),
            older: (shown < total).then_some(shown),
        })
    }

    async fn ip_detail(
        State(state): State<Arc<AppState>>,
        Path(addr): Path<String>,
    ) -> Result<Html<String>, AppError> {
        let window = state.window();
        let net = Allowlist::parse_cidr(&addr)?;
        let cidr = net.to_string();

        let rules = state.loki.rules_for_ip(&addr, &window).await?;
        let uris = state.loki.uris_for_ip(&addr, &window).await?;
        let lines = state.loki.recent_lines(&addr, &window, 100).await?;

        let protected = state.ctx.allowlist.entries().await;
        let block = Self::block_for(&state, &net).await?;

        let enrichment = match &state.enricher {
            Some(enricher) => Some(enricher.lookup(net.addr()).await),
            None => None,
        };

        Self::render(IpTemplate {
            client_ip: addr,
            window: state.window_label(),
            strikes: state.engine.strikes(&net).await.unwrap_or(0),
            protected: Allowlist::matching(&protected, &net)
                .map(|(range, source)| format!("{range} ({source})")),
            block,
            decisions: state.engine.decisions_for(&cidr).await?,
            audit: state.audit.for_target(&cidr, IP_HISTORY_LIMIT).await?,
            enrichment,
            cidr,
            rules: rules
                .into_iter()
                .map(|r| RuleRow {
                    ignored: state.ignored_rules().contains(&r.rule_id),
                    rule_id: r.rule_id,
                    rule_msg: r.rule_msg,
                    severity: r.severity,
                    count: r.count,
                })
                .collect(),
            uris,
            lines,
        })
    }

    async fn block_for(state: &AppState, net: &IpNet) -> Result<Option<BlockRow>, AppError> {
        let blocks = state
            .ctx
            .blocks()
            .list(&ListParams::default())
            .await
            .map_err(Error::from)?
            .items;

        let strikes = state.engine.all_strikes().await.unwrap_or_default();

        Ok(blocks
            .into_iter()
            .find(|b| {
                b.spec
                    .cidr
                    .parse::<IpNet>()
                    .is_ok_and(|blocked| blocked.contains(net))
            })
            .map(|b| BlockRow::new(b, &strikes)))
    }

    async fn audit(
        State(state): State<Arc<AppState>>,
        Query(page): Query<AuditQuery>,
    ) -> Result<Html<String>, AppError> {
        let offset = page.offset.max(0);
        let (entries, total) = state.audit.recent(AUDIT_PAGE_SIZE, offset).await?;
        let shown = offset + entries.len() as i64;

        Self::render(AuditTemplate {
            entries,
            total,
            newer: (offset > 0).then(|| (offset - AUDIT_PAGE_SIZE).max(0)),
            older: (shown < total).then_some(shown),
        })
    }

    async fn workflows(
        State(state): State<Arc<AppState>>,
        Query(query): Query<WorkflowsQuery>,
    ) -> Result<Html<String>, AppError> {
        let config = state.engine.config();
        let selected = Some(query.workflow.trim())
            .filter(|name| !name.is_empty())
            .map(str::to_string);
        let rows = config
            .workflows
            .iter()
            .map(|w| WorkflowRow {
                name: w.name.clone(),
                enabled: w.enabled.as_str(),
                tier: w.tier.as_str(),
                window: w.window(&config.defaults).to_string(),
                duration: w.duration.to_string(),
                gateway: w.gateway(&config.defaults).to_string(),
                reason: w.reason.clone(),
                signals: w.signals.keys().cloned().collect::<Vec<_>>().join(", "),
            })
            .collect();

        Self::render(WorkflowsTemplate {
            rows,
            scorecard: state.engine.scorecard().await?,
            decisions: state.engine.decisions(selected.as_deref()).await?,
            selected,
            cooldown: config.manual_unblock_cooldown.to_string(),
            protected: Self::protected_rows(&state.ctx.allowlist.entries().await),
            manual: state.manual.list().await?,
        })
    }

    fn protected_rows(entries: &[(IpNet, String)]) -> Vec<ProtectedRow> {
        let mut by_source: BTreeMap<&str, Vec<String>> = BTreeMap::new();
        for (net, why) in entries {
            by_source
                .entry(why.as_str())
                .or_default()
                .push(net.to_string());
        }

        const SHOWN: usize = 6;

        by_source
            .into_iter()
            .map(|(source, cidrs)| {
                let mut examples = cidrs.iter().take(SHOWN).cloned().collect::<Vec<_>>();
                if let Some(rest) = cidrs.len().checked_sub(SHOWN).filter(|r| *r > 0) {
                    examples.push(format!("and {rest} more"));
                }

                ProtectedRow {
                    source: source.to_string(),
                    count: cidrs.len(),
                    examples: examples.join(", "),
                }
            })
            .collect()
    }

    async fn create_block(
        State(state): State<Arc<AppState>>,
        Extension(claims): Extension<Claims>,
        Form(form): Form<BlockForm>,
    ) -> Result<Redirect, AppError> {
        let net = state.ctx.allowlist.parse_and_check(&form.cidr).await?;

        let expires_at = form.ttl_hours.filter(|h| *h > 0).map(|hours| {
            (chrono::Utc::now() + chrono::Duration::hours(hours as i64))
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
        });

        let gateway = form
            .gateway
            .unwrap_or_else(|| waf_manager::crd::DEFAULT_GATEWAY.to_string());

        let spec = WafBlockSpec {
            cidr: net.to_string(),
            gateway: gateway.clone(),
            reason: form.reason,
            rule_ids: None,
            expires_at,
            created_by: Some(claims.identity()),
        };

        let block = WafBlock::new(&WafBlock::resource_name(&net), spec);
        state
            .ctx
            .blocks()
            .create(&PostParams::default(), &block)
            .await
            .map_err(Error::from)?;

        state
            .audit
            .record(
                &claims.identity(),
                audit::ACTION_BLOCK,
                &net.to_string(),
                block.spec.reason.as_deref(),
            )
            .await;

        if *state.leadership.borrow() {
            state.ctx.sync_all().await?;
        }

        Ok(Redirect::to("/blocks"))
    }

    async fn delete_block(
        State(state): State<Arc<AppState>>,
        Extension(claims): Extension<Claims>,
        Form(form): Form<UnblockForm>,
    ) -> Result<Redirect, AppError> {
        let existing = state
            .ctx
            .blocks()
            .get_opt(&form.name)
            .await
            .map_err(Error::from)?;

        let cidr = existing
            .as_ref()
            .and_then(|b| b.spec.cidr.parse::<IpNet>().ok());

        let author = existing.as_ref().and_then(|b| {
            b.spec
                .created_by
                .as_deref()
                .and_then(|by| by.strip_prefix(WORKFLOW_AUTHOR_PREFIX))
                .map(str::to_string)
        });

        state
            .ctx
            .blocks()
            .delete(&form.name, &DeleteParams::default())
            .await
            .map_err(Error::from)?;

        if let Some(net) = cidr {
            state
                .audit
                .record(
                    &claims.identity(),
                    audit::ACTION_UNBLOCK,
                    &net.to_string(),
                    author.as_deref(),
                )
                .await;

            if let Some(workflow) = &author {
                state.engine.record_unblock(workflow, &net).await;
            }

            if let Err(e) = state.engine.suppress(&net).await {
                tracing::warn!("recording unblock suppression for {net} failed: {e}");
            }

            if let Err(e) = state.engine.forgive(&net).await {
                tracing::warn!("resetting offenses for {net} failed: {e}");
            }
        }

        if *state.leadership.borrow() {
            state.ctx.sync_all().await?;
        }

        Ok(Redirect::to("/blocks"))
    }

    async fn create_allowlist(
        State(state): State<Arc<AppState>>,
        Extension(claims): Extension<Claims>,
        Form(form): Form<AllowlistForm>,
    ) -> Result<Redirect, AppError> {
        let net = Allowlist::parse_cidr(&form.cidr)?;
        let blocked = Self::blocked_cidrs(&state).await?;

        if let Some(clash) = blocked
            .iter()
            .find(|b| b.contains(&net) || net.contains(*b))
        {
            return Err(AppError(Error::AllowlistConflict(
                net.to_string(),
                clash.to_string(),
            )));
        }

        state
            .manual
            .add(&net, form.note.as_deref(), &claims.identity())
            .await?;

        state
            .audit
            .record(
                &claims.identity(),
                audit::ACTION_ALLOWLIST_ADD,
                &net.to_string(),
                form.note.as_deref(),
            )
            .await;

        Ok(Redirect::to("/workflows#allowlist"))
    }

    async fn delete_allowlist(
        State(state): State<Arc<AppState>>,
        Extension(claims): Extension<Claims>,
        Form(form): Form<AllowlistDeleteForm>,
    ) -> Result<Redirect, AppError> {
        state.manual.remove(&form.cidr).await?;

        state
            .audit
            .record(
                &claims.identity(),
                audit::ACTION_ALLOWLIST_REMOVE,
                &form.cidr,
                None,
            )
            .await;

        Ok(Redirect::to("/workflows#allowlist"))
    }

    async fn blocked_cidrs(state: &AppState) -> Result<Vec<IpNet>, AppError> {
        let blocks = state
            .ctx
            .blocks()
            .list(&ListParams::default())
            .await
            .map_err(Error::from)?
            .items;

        Ok(blocks
            .iter()
            .filter_map(|b| waf_manager::Allowlist::parse_cidr(&b.spec.cidr).ok())
            .collect())
    }

    fn render<T: Template>(template: T) -> Result<Html<String>, AppError> {
        template
            .render()
            .map(Html)
            .map_err(|e| AppError(Error::Render(e.to_string())))
    }
}

#[derive(Deserialize)]
pub struct BlockForm {
    cidr: String,
    #[serde(default, deserialize_with = "blank_as_none")]
    gateway: Option<String>,
    #[serde(default, deserialize_with = "blank_as_none")]
    reason: Option<String>,
    #[serde(default, deserialize_with = "blank_as_none")]
    ttl_hours: Option<u32>,
}

fn blank_as_none<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    let raw = Option::<String>::deserialize(deserializer)?;
    match raw.as_deref().map(str::trim) {
        None | Some("") => Ok(None),
        Some(value) => value.parse().map(Some).map_err(serde::de::Error::custom),
    }
}

#[derive(Deserialize)]
pub struct UnblockForm {
    name: String,
}

#[derive(Deserialize)]
pub struct AllowlistForm {
    cidr: String,
    #[serde(default, deserialize_with = "blank_as_none")]
    note: Option<String>,
}

#[derive(Deserialize)]
pub struct AllowlistDeleteForm {
    cidr: String,
}

pub struct AppError(Error);

impl From<Error> for AppError {
    fn from(e: Error) -> Self {
        Self(e)
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = match self.0 {
            Error::ProtectedRange(..) | Error::InvalidCidr(..) => StatusCode::BAD_REQUEST,
            Error::Unauthorized(_) => StatusCode::UNAUTHORIZED,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };

        tracing::warn!("request failed: {}", self.0);
        (status, self.0.to_string()).into_response()
    }
}

pub struct CandidateRow {
    pub client_ip: String,
    pub detections: u64,
}

impl CandidateRow {
    fn new(client_ip: String, detections: u64) -> Self {
        Self {
            client_ip,
            detections,
        }
    }
}

pub struct ProtectedCandidateRow {
    pub client_ip: String,
    pub detections: u64,
    pub range: String,
    pub source: String,
}

pub struct BlockRow {
    pub name: String,
    pub automatic: bool,
    pub created_at: String,
    pub cidr: String,
    pub gateway: String,
    pub reason: String,
    pub created_by: String,
    pub expires_at: String,
    pub enforced: bool,
    pub strikes: i32,
}

impl BlockRow {
    fn new(block: WafBlock, strikes: &BTreeMap<String, i32>) -> Self {
        Self {
            strikes: block
                .spec
                .cidr
                .parse::<IpNet>()
                .ok()
                .and_then(|net| strikes.get(&net.to_string()).copied())
                .unwrap_or(0),
            name: block.name_any(),
            cidr: block.spec.cidr.clone(),
            gateway: block.spec.gateway.clone(),
            reason: block.spec.reason.clone().unwrap_or_default(),
            created_at: block
                .creation_timestamp()
                .map(|t| t.0.to_string())
                .unwrap_or_default(),
            automatic: block
                .spec
                .created_by
                .as_deref()
                .is_some_and(|by| by.starts_with(WORKFLOW_AUTHOR_PREFIX)),
            created_by: block.spec.created_by.clone().unwrap_or_default(),
            expires_at: block.spec.expires_at.clone().unwrap_or_default(),
            enforced: block
                .status
                .as_ref()
                .map(|s| {
                    s.conditions
                        .iter()
                        .any(|c| c.type_ == "Enforced" && c.status == "True")
                })
                .unwrap_or(false),
        }
    }
}

pub struct RuleRow {
    pub rule_id: String,
    pub rule_msg: String,
    pub severity: String,
    pub count: u64,
    pub ignored: bool,
}

#[derive(Template)]
#[template(path = "index.html")]
struct IndexTemplate {
    actionable: Vec<CandidateRow>,
    protected: Vec<ProtectedCandidateRow>,
    blocked: Vec<CandidateRow>,
    window: String,
}

pub struct WorkflowRow {
    pub name: String,
    pub enabled: &'static str,
    pub tier: &'static str,
    pub window: String,
    pub duration: String,
    pub gateway: String,
    pub reason: String,
    pub signals: String,
}

#[derive(Template)]
#[template(path = "workflows.html")]
struct WorkflowsTemplate {
    rows: Vec<WorkflowRow>,
    scorecard: Vec<Scorecard>,
    decisions: Vec<Decision>,
    selected: Option<String>,
    cooldown: String,
    protected: Vec<ProtectedRow>,
    manual: Vec<ManualEntry>,
}

#[derive(Debug, Default, Deserialize)]
pub struct WorkflowsQuery {
    #[serde(default)]
    pub workflow: String,
}

pub struct ProtectedRow {
    pub source: String,
    pub count: usize,
    pub examples: String,
}

#[derive(Template)]
#[template(path = "blocks.html")]
struct BlocksTemplate {
    rows: Vec<BlockRow>,
    conflicts: Vec<Conflict>,
    total: usize,
    offset: usize,
    query: String,
    newer: Option<usize>,
    older: Option<usize>,
}

#[derive(Debug, Default, Deserialize)]
pub struct BlocksQuery {
    #[serde(default)]
    pub q: String,

    #[serde(default)]
    pub offset: usize,
}

#[derive(Template)]
#[template(path = "ip.html")]
struct IpTemplate {
    client_ip: String,
    cidr: String,
    window: String,
    strikes: i32,
    protected: Option<String>,
    block: Option<BlockRow>,
    decisions: Vec<Decision>,
    audit: Vec<waf_manager::audit::Entry>,
    enrichment: Option<Arc<Enrichment>>,
    rules: Vec<RuleRow>,
    uris: Vec<waf_manager::UriHit>,
    lines: Vec<String>,
}

#[derive(Template)]
#[template(path = "audit.html")]
struct AuditTemplate {
    entries: Vec<waf_manager::audit::Entry>,
    total: i64,
    newer: Option<i64>,
    older: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
pub struct AuditQuery {
    #[serde(default)]
    pub offset: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(body: &str) -> BlockForm {
        serde_urlencoded::from_str(body).expect(body)
    }

    fn decision(workflow: &str, cidr: &str, mode: &str) -> Decision {
        Decision {
            at: chrono::Utc::now(),
            workflow: workflow.to_string(),
            cidr: cidr.to_string(),
            detections: 7,
            mode: mode.to_string(),
            outcome: "would block".to_string(),
        }
    }

    fn workflows_page(selected: Option<&str>, decisions: Vec<Decision>) -> String {
        page_with(selected, decisions, Vec::new(), Vec::new())
    }

    fn page_with(
        selected: Option<&str>,
        decisions: Vec<Decision>,
        scorecard: Vec<Scorecard>,
        manual: Vec<ManualEntry>,
    ) -> String {
        WorkflowsTemplate {
            rows: vec![WorkflowRow {
                name: "scanner burst".to_string(),
                enabled: "dry-run",
                tier: "standard",
                window: "12h".to_string(),
                duration: "24h".to_string(),
                gateway: "public-gateway".to_string(),
                reason: "burst".to_string(),
                signals: String::new(),
            }],
            scorecard,
            decisions,
            selected: selected.map(str::to_string),
            cooldown: "24h".to_string(),
            protected: Vec::new(),
            manual,
        }
        .render()
        .unwrap()
    }

    #[test]
    fn workflow_names_link_to_their_own_decisions() {
        let page = workflows_page(None, Vec::new());

        assert!(page.contains(r#"href="/workflows?workflow=scanner%20burst#decisions""#));
        assert!(page.contains("Click a workflow name above"));
    }

    #[test]
    fn a_selected_workflow_keeps_its_dry_run_decisions() {
        let page = workflows_page(
            Some("scanner burst"),
            vec![
                decision("scanner burst", "203.0.113.4/32", "dry-run"),
                decision("scanner burst", "203.0.113.5/32", "active"),
            ],
        );

        assert!(page.contains("dry-run decisions included"));
        assert!(page.contains("Show all workflows"));
        assert!(page.contains("203.0.113.4/32"));
        assert!(page.contains("203.0.113.5/32"));
    }

    #[test]
    fn a_selected_workflow_with_no_decisions_says_so() {
        let page = workflows_page(Some("scanner burst"), Vec::new());

        assert!(page.contains("has not decided anything yet"));
        assert!(!page.contains("Nothing decided since this pod started"));
    }

    #[test]
    fn blank_optional_fields_are_none() {
        let form = parse("cidr=203.0.113.4&reason=&ttl_hours=");

        assert_eq!(form.cidr, "203.0.113.4");
        assert!(form.reason.is_none());
        assert!(form.ttl_hours.is_none());
    }

    #[test]
    fn absent_optional_fields_are_none() {
        let form = parse("cidr=203.0.113.4");
        assert!(form.ttl_hours.is_none());
    }

    #[test]
    fn populated_fields_parse() {
        let form = parse("cidr=203.0.113.4&reason=probing&ttl_hours=24");

        assert_eq!(form.reason.as_deref(), Some("probing"));
        assert_eq!(form.ttl_hours, Some(24));
    }

    #[test]
    fn whitespace_only_counts_as_blank() {
        let form = parse("cidr=203.0.113.4&reason=%20%20&ttl_hours=%20");

        assert!(form.reason.is_none());
        assert!(form.ttl_hours.is_none());
    }

    #[test]
    fn a_real_non_numeric_ttl_is_still_an_error() {
        let err = serde_urlencoded::from_str::<BlockForm>("cidr=203.0.113.4&ttl_hours=soon");
        assert!(err.is_err());
    }
}
