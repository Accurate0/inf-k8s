use askama::Template;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Form, Router};
use ipnet::IpNet;
use kube::ResourceExt;
use kube::api::{DeleteParams, ListParams, PostParams};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use waf_manager::compositor::Conflict;
use waf_manager::controller::Context;
use waf_manager::crd::{WafBlock, WafBlockSpec};
use waf_manager::metrics::Metrics;
use waf_manager::workflows::{Decision, WORKFLOW_AUTHOR_PREFIX};
use waf_manager::{Error, Loki, WorkflowEngine};

pub struct AppState {
    pub ctx: Arc<Context>,
    pub loki: Arc<Loki>,
    pub engine: Arc<WorkflowEngine>,
}

impl AppState {
    fn window(&self) -> String {
        self.engine.config().defaults.window.to_logql()
    }

    /// How the window is written in config.yaml, for the page headings.
    fn window_label(&self) -> String {
        self.engine.config().defaults.window.to_string()
    }

    /// Noise rather than findings; hidden from the ranking.
    fn ignored_rules(&self) -> &BTreeSet<String> {
        &self.engine.config().ignored_rule_ids
    }
}

pub struct Routes;

impl Routes {
    pub fn router(state: Arc<AppState>) -> Router {
        Router::new()
            .route("/", get(Self::index))
            .route("/blocks", get(Self::blocks))
            .route("/workflows", get(Self::workflows))
            .route("/ip/{addr}", get(Self::ip_detail))
            .route("/block", post(Self::create_block))
            .route("/unblock", post(Self::delete_block))
            .route("/health", get(|| async { StatusCode::OK }))
            .route("/metrics", get(|| async { Metrics::render() }))
            .with_state(state)
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
        let rows = candidates
            .into_iter()
            .map(|c| {
                let net = waf_manager::Allowlist::parse_cidr(&c.client_ip).ok();
                let already_blocked = net
                    .map(|net| blocked.iter().any(|b| b.contains(&net)))
                    .unwrap_or(false);

                CandidateRow {
                    client_ip: c.client_ip,
                    detections: c.detections,
                    already_blocked,
                    // Blocking one is refused, so say so before the form is used.
                    protected: net
                        .and_then(|net| waf_manager::Allowlist::overlap(&protected, &net))
                        .unwrap_or_default(),
                }
            })
            .collect();

        Self::render(IndexTemplate {
            rows,
            window: state.window_label(),
        })
    }

    async fn blocks(State(state): State<Arc<AppState>>) -> Result<Html<String>, AppError> {
        let mut blocks = state
            .ctx
            .blocks()
            .list(&ListParams::default())
            .await
            .map_err(Error::from)?
            .items;

        // Newest first: the block someone just added, or a workflow just made,
        // is the one being looked for. The API lists by name, which is the CIDR.
        blocks.sort_by(|a, b| {
            b.creation_timestamp()
                .cmp(&a.creation_timestamp())
                .then_with(|| a.name_any().cmp(&b.name_any()))
        });

        let rows = blocks
            .into_iter()
            .map(|b| BlockRow {
                name: b.name_any(),
                cidr: b.spec.cidr.clone(),
                gateway: b.spec.gateway.clone(),
                reason: b.spec.reason.clone().unwrap_or_default(),
                created_at: b
                    .creation_timestamp()
                    .map(|t| t.0.to_string())
                    .unwrap_or_default(),
                automatic: b
                    .spec
                    .created_by
                    .as_deref()
                    .is_some_and(|by| by.starts_with(WORKFLOW_AUTHOR_PREFIX)),
                created_by: b.spec.created_by.clone().unwrap_or_default(),
                expires_at: b.spec.expires_at.clone().unwrap_or_default(),
                enforced: b
                    .status
                    .as_ref()
                    .map(|s| {
                        s.conditions
                            .iter()
                            .any(|c| c.type_ == "Enforced" && c.status == "True")
                    })
                    .unwrap_or(false),
            })
            .collect();

        Self::render(BlocksTemplate {
            rows,
            conflicts: state.ctx.conflicts().await,
        })
    }

    async fn ip_detail(
        State(state): State<Arc<AppState>>,
        Path(addr): Path<String>,
    ) -> Result<Html<String>, AppError> {
        let rules = state.loki.rules_for_ip(&addr, &state.window()).await?;
        let lines = state.loki.recent_lines(&addr, &state.window(), 100).await?;

        Self::render(IpTemplate {
            client_ip: addr,
            window: state.window_label(),
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
            lines,
        })
    }

    async fn workflows(State(state): State<Arc<AppState>>) -> Result<Html<String>, AppError> {
        let config = state.engine.config();
        let rows = config
            .workflows
            .iter()
            .map(|w| WorkflowRow {
                name: w.name.clone(),
                enabled: w.enabled.as_str(),
                window: w.window(&config.defaults).to_string(),
                duration: w.duration.to_string(),
                gateway: w.gateway(&config.defaults).to_string(),
                reason: w.reason.clone(),
                signals: w.signals.keys().cloned().collect::<Vec<_>>().join(", "),
            })
            .collect();

        Self::render(WorkflowsTemplate {
            rows,
            decisions: state.engine.decisions().await,
            cooldown: config.manual_unblock_cooldown.to_string(),
            // A feed can carry thousands of ranges, so summarise by source
            // rather than rendering every CIDR.
            protected: Self::protected_rows(&state.ctx.allowlist.entries().await),
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
        headers: HeaderMap,
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
            created_by: Self::user(&headers),
        };

        // The reconciler sets the owner, so git-applied blocks get one too.
        let block = WafBlock::new(&WafBlock::resource_name(&net), spec);
        state
            .ctx
            .blocks()
            .create(&PostParams::default(), &block)
            .await
            .map_err(Error::from)?;

        Ok(Redirect::to("/blocks"))
    }

    /// Envoy injects these from OIDC claims; they never reach here from the
    /// client, because the gateway sets them. `forwardAccessToken` forwards the
    /// access token, which does not always carry `preferred_username` - `sub` is
    /// the only claim guaranteed to be there.
    fn user(headers: &HeaderMap) -> Option<String> {
        const CLAIM_HEADERS: &[&str] = &[
            "x-waf-user",
            "x-waf-user-name",
            "x-waf-user-email",
            "x-waf-user-sub",
        ];

        let found = CLAIM_HEADERS.iter().find_map(|name| {
            headers
                .get(*name)
                .and_then(|v| v.to_str().ok())
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_string)
        });

        if found.is_none() {
            let seen: Vec<&str> = headers
                .keys()
                .map(|k| k.as_str())
                .filter(|k| k.starts_with("x-waf-") || *k == "authorization")
                .collect();
            tracing::warn!("no identity claim header on request; saw {seen:?}");
        }

        found
    }

    async fn delete_block(
        State(state): State<Arc<AppState>>,
        Form(form): Form<UnblockForm>,
    ) -> Result<Redirect, AppError> {
        // Read the CIDR before deleting: afterwards there is nothing left to
        // derive it from, and without it a workflow re-creates the block the
        // operator just removed.
        let cidr = state
            .ctx
            .blocks()
            .get_opt(&form.name)
            .await
            .map_err(Error::from)?
            .and_then(|b| b.spec.cidr.parse::<IpNet>().ok());

        state
            .ctx
            .blocks()
            .delete(&form.name, &DeleteParams::default())
            .await
            .map_err(Error::from)?;

        if let Some(net) = cidr
            && let Err(e) = state.engine.suppress(&net).await
        {
            // The unblock itself succeeded; a failed suppression only risks the
            // block coming back on the next tick.
            tracing::warn!("recording unblock suppression for {net} failed: {e}");
        }

        // The watch would catch up anyway; inline means the redirect lands on a
        // page that already reflects the change.
        state.ctx.sync_all().await?;

        Ok(Redirect::to("/blocks"))
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
            .map_err(|e| AppError(Error::Loki(format!("template failed: {e}"))))
    }
}

#[derive(Deserialize)]
pub struct BlockForm {
    cidr: String,
    #[serde(default, deserialize_with = "blank_as_none")]
    gateway: Option<String>,
    #[serde(default, deserialize_with = "blank_as_none")]
    reason: Option<String>,
    /// An untouched number input still posts `ttl_hours=`, which is not a u32.
    #[serde(default, deserialize_with = "blank_as_none")]
    ttl_hours: Option<u32>,
}

/// HTML forms submit every field, so an empty control arrives as an empty string
/// rather than being absent. Serde would reject that for any non-string type.
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

pub struct AppError(Error);

impl From<Error> for AppError {
    fn from(e: Error) -> Self {
        Self(e)
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = match self.0 {
            // Operator error, not a server fault.
            Error::ProtectedRange(..) | Error::InvalidCidr(..) => StatusCode::BAD_REQUEST,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };

        tracing::warn!("request failed: {}", self.0);
        (status, self.0.to_string()).into_response()
    }
}

pub struct CandidateRow {
    pub client_ip: String,
    pub detections: u64,
    pub already_blocked: bool,
    /// `"<cidr> (<source>)"` when the IP falls in a protected range, else empty.
    pub protected: String,
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
    rows: Vec<CandidateRow>,
    window: String,
}

pub struct WorkflowRow {
    pub name: String,
    pub enabled: &'static str,
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
    decisions: Vec<Decision>,
    cooldown: String,
    protected: Vec<ProtectedRow>,
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
}

#[derive(Template)]
#[template(path = "ip.html")]
struct IpTemplate {
    client_ip: String,
    window: String,
    rules: Vec<RuleRow>,
    lines: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(body: &str) -> BlockForm {
        serde_urlencoded::from_str(body).expect(body)
    }

    #[test]
    fn blank_optional_fields_are_none() {
        // Exactly what the form posts when reason and ttl are left untouched.
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
