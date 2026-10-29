use crate::routes::{AppError, AppState, CandidateRow, Routes};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::middleware::from_fn_with_state;
use axum::routing::{delete, get, post};
use axum::{Extension, Json, Router};
use kube::ResourceExt;
use kube::api::ListParams;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::sync::Arc;
use waf_manager::workflows::WORKFLOW_AUTHOR_PREFIX;
use waf_manager::{Allowlist, Claims, Error, WafBlock};
use waf_manager_api as api;

const IP_HISTORY_LIMIT: i64 = 20;
const AUDIT_PAGE_SIZE: i64 = 100;

pub struct Api;

impl Api {
    pub fn router(state: Arc<AppState>) -> Router {
        Router::new()
            .route("/api/v1/candidates", get(Self::candidates))
            .route("/api/v1/blocks", get(Self::blocks).post(Self::create_block))
            .route("/api/v1/blocks/{name}", delete(Self::delete_block))
            .route(
                "/api/v1/blocks/{name}/permanent",
                post(Self::make_permanent),
            )
            .route("/api/v1/ip/{addr}", get(Self::ip))
            .route("/api/v1/audit", get(Self::audit))
            .route("/api/v1/workflows", get(Self::workflows))
            .route(
                "/api/v1/allowlist",
                get(Self::allowlist)
                    .post(Self::create_allowlist)
                    .delete(Self::delete_allowlist),
            )
            .layer(from_fn_with_state(state.clone(), Routes::authenticate))
            .with_state(state)
    }

    async fn candidates(
        State(state): State<Arc<AppState>>,
    ) -> Result<Json<api::Candidates>, AppError> {
        let (actionable, protected, blocked) = Routes::candidates(&state).await?;
        let plain = |rows: Vec<CandidateRow>| {
            rows.into_iter()
                .map(|c| api::Candidate {
                    client_ip: c.client_ip,
                    detections: c.detections,
                    protected_by: None,
                })
                .collect()
        };

        Ok(Json(api::Candidates {
            window: state.window_label(),
            actionable: plain(actionable),
            protected: protected
                .into_iter()
                .map(|c| api::Candidate {
                    client_ip: c.client_ip,
                    detections: c.detections,
                    protected_by: Some(format!("{} ({})", c.range, c.source)),
                })
                .collect(),
            blocked: plain(blocked),
        }))
    }

    async fn blocks(
        State(state): State<Arc<AppState>>,
        Query(query): Query<BlocksQuery>,
    ) -> Result<Json<api::Blocks>, AppError> {
        let mut blocks = Self::all_blocks(&state).await?;
        let filter = query.q.trim().to_lowercase();
        if !filter.is_empty() {
            blocks.retain(|b| b.cidr.to_lowercase().contains(&filter));
        }

        Ok(Json(api::Blocks {
            blocks,
            conflicts: state
                .ctx
                .conflicts()
                .await?
                .into_iter()
                .map(|c| api::Conflict {
                    source: c.source,
                    message: c.message,
                })
                .collect(),
        }))
    }

    async fn create_block(
        State(state): State<Arc<AppState>>,
        Extension(claims): Extension<Claims>,
        Json(request): Json<api::CreateBlock>,
    ) -> Result<(StatusCode, Json<api::Block>), AppError> {
        let block = Routes::block(
            &state,
            &claims,
            &request.cidr,
            request.gateway.filter(|g| !g.trim().is_empty()),
            request.reason.filter(|r| !r.trim().is_empty()),
            request.ttl_hours,
        )
        .await?;

        Ok((StatusCode::CREATED, Json(to_block(block, &BTreeMap::new()))))
    }

    async fn delete_block(
        State(state): State<Arc<AppState>>,
        Extension(claims): Extension<Claims>,
        Path(name): Path<String>,
    ) -> Result<StatusCode, AppError> {
        Routes::unblock(&state, &claims, &name).await?;
        Ok(StatusCode::NO_CONTENT)
    }

    async fn make_permanent(
        State(state): State<Arc<AppState>>,
        Extension(claims): Extension<Claims>,
        Path(name): Path<String>,
    ) -> Result<StatusCode, AppError> {
        match Routes::make_permanent(&state, &claims, &name).await? {
            Some(_) => Ok(StatusCode::NO_CONTENT),
            None => Ok(StatusCode::NOT_MODIFIED),
        }
    }

    async fn ip(
        State(state): State<Arc<AppState>>,
        Path(addr): Path<String>,
    ) -> Result<Json<api::IpDetail>, AppError> {
        let window = state.window();
        let net = Allowlist::parse_cidr(&addr)?;
        let cidr = net.to_string();

        let rules = state.loki.rules_for_ip(&addr, &window).await?;
        let uris = state.loki.uris_for_ip(&addr, &window).await?;
        let lines = state.loki.recent_lines(&addr, &window, 100).await?;
        let protected = state.ctx.allowlist.entries().await;
        let strikes = state.engine.all_strikes().await.unwrap_or_default();

        let block = state
            .ctx
            .blocks()
            .list(&ListParams::default())
            .await
            .map_err(Error::from)?
            .items
            .into_iter()
            .find(|b| {
                b.spec
                    .cidr
                    .parse::<ipnet::IpNet>()
                    .is_ok_and(|blocked| blocked.contains(&net))
            })
            .map(|b| to_block(b, &strikes));

        let enrichment = match &state.enricher {
            Some(enricher) => {
                let e = enricher.lookup(net.addr()).await;
                Some(api::Enrichment {
                    hostname: e.hostname.clone(),
                    asn: e.asn.clone(),
                    org: e.org.clone(),
                    prefix: e.prefix.clone(),
                    country: e.country.clone(),
                })
            }
            None => None,
        };

        Ok(Json(api::IpDetail {
            client_ip: addr,
            window: state.window_label(),
            strikes: state.engine.strikes(&net).await.unwrap_or(0),
            protected: Allowlist::matching(&protected, &net)
                .map(|(range, source)| format!("{range} ({source})")),
            block,
            decisions: state
                .engine
                .decisions_for(&cidr)
                .await?
                .into_iter()
                .map(to_decision)
                .collect(),
            audit: state
                .audit
                .for_target(&cidr, IP_HISTORY_LIMIT)
                .await?
                .into_iter()
                .map(to_audit)
                .collect(),
            enrichment,
            cidr,
            rules: rules
                .into_iter()
                .map(|r| api::Rule {
                    ignored: state.ignored_rules().contains(&r.rule_id),
                    rule_id: r.rule_id,
                    rule_msg: r.rule_msg,
                    severity: r.severity,
                    count: r.count,
                })
                .collect(),
            uris: uris
                .into_iter()
                .map(|u| api::UriHit {
                    uri: u.uri,
                    count: u.count,
                })
                .collect(),
            lines,
        }))
    }

    async fn audit(
        State(state): State<Arc<AppState>>,
        Query(query): Query<AuditQuery>,
    ) -> Result<Json<api::AuditPage>, AppError> {
        let limit = query.limit.unwrap_or(AUDIT_PAGE_SIZE).clamp(1, 1000);
        let (entries, total) = state.audit.recent(limit, query.offset.max(0)).await?;

        Ok(Json(api::AuditPage {
            entries: entries.into_iter().map(to_audit).collect(),
            total,
        }))
    }

    async fn workflows(
        State(state): State<Arc<AppState>>,
        Query(query): Query<WorkflowsQuery>,
    ) -> Result<Json<api::Workflows>, AppError> {
        let config = state.engine.config();
        let selected = query
            .workflow
            .as_deref()
            .map(str::trim)
            .filter(|w| !w.is_empty());

        Ok(Json(api::Workflows {
            workflows: config
                .workflows
                .iter()
                .map(|w| api::Workflow {
                    name: w.name.clone(),
                    enabled: w.enabled.as_str().to_owned(),
                    tier: w.tier.as_str().to_owned(),
                    window: w.window(&config.defaults).to_string(),
                    duration: w.duration.to_string(),
                    gateway: w.gateway(&config.defaults).to_string(),
                    reason: w.reason.clone(),
                    signals: w.signals.keys().cloned().collect(),
                })
                .collect(),
            scorecard: state
                .engine
                .scorecard()
                .await?
                .into_iter()
                .map(|s| api::Scorecard {
                    workflow: s.workflow,
                    enabled: s.enabled.to_owned(),
                    decisions: s.decisions,
                    cidrs: s.cidrs,
                    unblocked: s.unblocked,
                    agreement: s.agreement,
                })
                .collect(),
            decisions: state
                .engine
                .decisions(selected)
                .await?
                .into_iter()
                .map(to_decision)
                .collect(),
            cooldown: config.manual_unblock_cooldown.to_string(),
        }))
    }

    async fn allowlist(
        State(state): State<Arc<AppState>>,
    ) -> Result<Json<api::Allowlist>, AppError> {
        let protected = Routes::protected_rows(&state.ctx.allowlist.entries().await);

        Ok(Json(api::Allowlist {
            manual: state
                .manual
                .list()
                .await?
                .into_iter()
                .map(|m| api::AllowlistEntry {
                    cidr: m.cidr,
                    note: m.note,
                    created_by: m.created_by,
                    at: m.at,
                })
                .collect(),
            protected: protected
                .into_iter()
                .map(|p| api::ProtectedSource {
                    source: p.source,
                    count: p.count,
                })
                .collect(),
        }))
    }

    async fn create_allowlist(
        State(state): State<Arc<AppState>>,
        Extension(claims): Extension<Claims>,
        Json(request): Json<api::CreateAllowlist>,
    ) -> Result<StatusCode, AppError> {
        let note = request.note.as_deref().filter(|n| !n.trim().is_empty());
        Routes::allow(&state, &claims, &request.cidr, note).await?;
        Ok(StatusCode::CREATED)
    }

    async fn delete_allowlist(
        State(state): State<Arc<AppState>>,
        Extension(claims): Extension<Claims>,
        Query(query): Query<CidrQuery>,
    ) -> Result<StatusCode, AppError> {
        let net = Allowlist::parse_cidr(&query.cidr)?;
        Routes::disallow(&state, &claims, &net.to_string()).await?;
        Ok(StatusCode::NO_CONTENT)
    }

    async fn all_blocks(state: &AppState) -> Result<Vec<api::Block>, AppError> {
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

        let strikes = state.engine.all_strikes().await.unwrap_or_default();
        Ok(blocks.into_iter().map(|b| to_block(b, &strikes)).collect())
    }
}

fn to_block(block: WafBlock, strikes: &BTreeMap<String, i32>) -> api::Block {
    let created_by = block.spec.created_by.clone();
    api::Block {
        strikes: block
            .spec
            .cidr
            .parse::<ipnet::IpNet>()
            .ok()
            .and_then(|net| strikes.get(&net.to_string()).copied())
            .unwrap_or(0),
        name: block.name_any(),
        created_at: block.creation_timestamp().and_then(|t| {
            chrono::DateTime::from_timestamp(t.0.as_second(), t.0.subsec_nanosecond() as u32)
        }),
        enforced: block.status.as_ref().is_some_and(|s| {
            s.conditions
                .iter()
                .any(|c| c.type_ == "Enforced" && c.status == "True")
        }),
        automatic: created_by
            .as_deref()
            .is_some_and(|by| by.starts_with(WORKFLOW_AUTHOR_PREFIX)),
        created_by: created_by.map(|by| {
            by.strip_prefix(WORKFLOW_AUTHOR_PREFIX)
                .map(str::to_owned)
                .unwrap_or(by)
        }),
        cidr: block.spec.cidr,
        gateway: block.spec.gateway,
        reason: block.spec.reason,
        expires_at: block.spec.expires_at,
    }
}

fn to_decision(d: waf_manager::Decision) -> api::Decision {
    api::Decision {
        at: d.at,
        workflow: d.workflow,
        cidr: d.cidr,
        detections: d.detections,
        mode: d.mode,
        outcome: d.outcome,
    }
}

fn to_audit(e: waf_manager::audit::Entry) -> api::AuditEntry {
    api::AuditEntry {
        at: e.at,
        actor: e.actor,
        action: e.action,
        target: e.target,
        detail: e.detail,
    }
}

#[derive(Debug, Default, Deserialize)]
struct BlocksQuery {
    #[serde(default)]
    q: String,
}

#[derive(Debug, Default, Deserialize)]
struct AuditQuery {
    #[serde(default)]
    offset: i64,
    #[serde(default)]
    limit: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
struct WorkflowsQuery {
    #[serde(default)]
    workflow: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CidrQuery {
    cidr: String,
}
