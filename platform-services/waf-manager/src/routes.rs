use askama::Template;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Form, Router};
use ipnet::IpNet;
use kube::ResourceExt;
use kube::api::{DeleteParams, ListParams, PostParams};
use serde::Deserialize;
use std::collections::BTreeSet;
use std::sync::Arc;
use waf_manager::compositor::Conflict;
use waf_manager::controller::Context;
use waf_manager::crd::{WafBlock, WafBlockSpec};
use waf_manager::metrics::Metrics;
use waf_manager::{Error, Loki};

pub struct AppState {
    pub ctx: Arc<Context>,
    pub loki: Loki,
    pub window: String,
    /// Noise rather than findings; hidden from the ranking.
    pub ignored_rules: BTreeSet<String>,
}

pub struct Routes;

impl Routes {
    pub fn router(state: Arc<AppState>) -> Router {
        Router::new()
            .route("/", get(Self::index))
            .route("/blocks", get(Self::blocks))
            .route("/ip/{addr}", get(Self::ip_detail))
            .route("/block", post(Self::create_block))
            .route("/unblock", post(Self::delete_block))
            .route("/health", get(|| async { StatusCode::OK }))
            .route("/metrics", get(|| async { Metrics::render() }))
            .with_state(state)
    }

    async fn index(State(state): State<Arc<AppState>>) -> Result<Html<String>, AppError> {
        let candidates = match state.loki.top_client_ips(&state.window, 50).await {
            Ok(c) => c,
            Err(e) => {
                Metrics::record_loki_error();
                return Err(e.into());
            }
        };

        let blocked = Self::blocked_cidrs(&state).await?;
        let rows = candidates
            .into_iter()
            .map(|c| {
                let already_blocked = waf_manager::Allowlist::parse_cidr(&c.client_ip)
                    .map(|net| blocked.iter().any(|b| b.contains(&net)))
                    .unwrap_or(false);

                CandidateRow {
                    client_ip: c.client_ip,
                    detections: c.detections,
                    already_blocked,
                }
            })
            .collect();

        Self::render(IndexTemplate {
            rows,
            window: state.window.clone(),
        })
    }

    async fn blocks(State(state): State<Arc<AppState>>) -> Result<Html<String>, AppError> {
        let blocks = state
            .ctx
            .blocks()
            .list(&ListParams::default())
            .await
            .map_err(Error::from)?
            .items;

        let rows = blocks
            .into_iter()
            .map(|b| BlockRow {
                name: b.name_any(),
                cidr: b.spec.cidr.clone(),
                gateway: b.spec.gateway.clone(),
                reason: b.spec.reason.clone().unwrap_or_default(),
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
        let rules = state.loki.rules_for_ip(&addr, &state.window).await?;
        let lines = state.loki.recent_lines(&addr, &state.window, 100).await?;

        Self::render(IpTemplate {
            client_ip: addr,
            window: state.window.clone(),
            rules: rules
                .into_iter()
                .map(|r| RuleRow {
                    ignored: state.ignored_rules.contains(&r.rule_id),
                    rule_id: r.rule_id,
                    rule_msg: r.rule_msg,
                    severity: r.severity,
                    count: r.count,
                })
                .collect(),
            lines,
        })
    }

    async fn create_block(
        State(state): State<Arc<AppState>>,
        Form(form): Form<BlockForm>,
    ) -> Result<Redirect, AppError> {
        let net = state.ctx.allowlist.parse_and_check(&form.cidr)?;

        let expires_at = form.ttl_hours.filter(|h| *h > 0).map(|hours| {
            (chrono::Utc::now() + chrono::Duration::hours(hours as i64))
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
        });

        let spec = WafBlockSpec {
            cidr: net.to_string(),
            gateway: form
                .gateway
                .unwrap_or_else(|| waf_manager::crd::DEFAULT_GATEWAY.to_string()),
            reason: form.reason.filter(|r| !r.trim().is_empty()),
            rule_ids: None,
            expires_at,
        };

        let block = WafBlock::new(&Self::resource_name(&net), spec);
        state
            .ctx
            .blocks()
            .create(&PostParams::default(), &block)
            .await
            .map_err(Error::from)?;

        Ok(Redirect::to("/blocks"))
    }

    async fn delete_block(
        State(state): State<Arc<AppState>>,
        Form(form): Form<UnblockForm>,
    ) -> Result<Redirect, AppError> {
        state
            .ctx
            .blocks()
            .delete(&form.name, &DeleteParams::default())
            .await
            .map_err(Error::from)?;

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

    /// Derived from the CIDR so a double submit collides rather than duplicating.
    fn resource_name(net: &IpNet) -> String {
        let mut slug = String::new();
        let mut last_dash = true;

        for ch in net.to_string().chars() {
            if ch.is_ascii_alphanumeric() {
                slug.push(ch.to_ascii_lowercase());
                last_dash = false;
            } else if !last_dash {
                slug.push('-');
                last_dash = true;
            }
        }

        let slug = slug.trim_matches('-');
        format!("waf-block-{slug}")
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
    gateway: Option<String>,
    reason: Option<String>,
    ttl_hours: Option<u32>,
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
}

pub struct BlockRow {
    pub name: String,
    pub cidr: String,
    pub gateway: String,
    pub reason: String,
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
