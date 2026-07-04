use axum::{
    extract::{Json, Path, State},
    http::StatusCode,
};
use serde::Serialize;
use std::sync::Arc;
use tracing::Instrument;

use super::background_span;
use crate::AppState;
use janitor_bot::{command, event};

pub async fn handle_admin_cron(State(state): State<Arc<AppState>>) -> StatusCode {
    tracing::info!("admin: triggering cron evaluation");
    tokio::spawn(
        async move {
            crate::evaluate_open_prs(&state).await;
        }
        .instrument(background_span("cron_open_prs")),
    );
    StatusCode::OK
}

pub async fn handle_admin_evaluate_pr(
    State(state): State<Arc<AppState>>,
    Path((owner, repo, pr_number)): Path<(String, String, i64)>,
) -> StatusCode {
    tracing::info!(
        owner,
        repo,
        pr_number,
        "admin: triggering evaluation for PR"
    );

    tokio::spawn(
        async move {
            let pr = match state.clients.forgejo.get_pr(&owner, &repo, pr_number).await {
                Ok(pr) => pr,
                Err(e) => {
                    tracing::error!(pr_number, "admin: failed to fetch PR: {e}");
                    return;
                }
            };

            let Some(mut pr_event) = janitor_bot::event::PrEvent::from_api_pr(&pr, owner, repo)
            else {
                tracing::error!(pr_number, "admin: failed to convert PR to event");
                return;
            };

            state
                .orchestrator
                .evaluate_pr(&state.clients, &mut pr_event)
                .await;
        }
        .instrument(background_span("admin_evaluate_pr")),
    );

    StatusCode::OK
}

pub async fn handle_admin_dry_run(
    State(state): State<Arc<AppState>>,
    Path((owner, repo, pr_number)): Path<(String, String, i64)>,
) -> (StatusCode, Json<serde_json::Value>) {
    tracing::info!(owner, repo, pr_number, "admin: dry-run for PR");

    let pr = match state.clients.forgejo.get_pr(&owner, &repo, pr_number).await {
        Ok(pr) => pr,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": format!("failed to fetch PR: {e}")})),
            );
        }
    };

    let Some(mut pr_event) = janitor_bot::event::PrEvent::from_api_pr(&pr, owner, repo) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "failed to convert PR to event"})),
        );
    };

    let matched = state
        .orchestrator
        .explain_pr(&state.clients, &mut pr_event)
        .await;

    (
        StatusCode::OK,
        Json(serde_json::to_value(&matched).unwrap()),
    )
}

pub async fn handle_admin_merge_queued(
    State(state): State<Arc<AppState>>,
) -> (StatusCode, Json<serde_json::Value>) {
    tracing::info!("admin: spawning background merge of all queued PRs");

    tokio::spawn(run_admin_merge_queued(state));

    (
        StatusCode::ACCEPTED,
        Json(serde_json::json!({"status": "started"})),
    )
}

async fn run_admin_merge_queued(state: Arc<AppState>) {
    tracing::info!("admin: merging all queued PRs");

    const QUEUED_LABEL: &str = "janitor/queued";

    let mut merged: Vec<serde_json::Value> = Vec::new();
    let mut failed: Vec<serde_json::Value> = Vec::new();

    for (owner, repo) in state.orchestrator.watch_repos() {
        let prs = match state.clients.forgejo.list_open_prs(owner, repo).await {
            Ok(prs) => prs,
            Err(e) => {
                failed.push(
                    serde_json::json!({"repo": repo, "error": format!("list_open_prs: {e}")}),
                );
                continue;
            }
        };

        for pr in prs {
            let Some(pr_number) = pr.number else {
                continue;
            };

            let has_queued = pr
                .labels
                .as_ref()
                .map(|ls| ls.iter().any(|l| l.name.as_deref() == Some(QUEUED_LABEL)))
                .unwrap_or(false);
            if !has_queued {
                continue;
            }

            if !state
                .clients
                .forgejo
                .is_pr_approved_by_bot(owner, repo, pr_number)
                .await
                && let Err(e) = state
                    .clients
                    .forgejo
                    .approve_pr(
                        owner,
                        repo,
                        pr_number,
                        Some("Auto-approved via /admin/merge-queued"),
                    )
                    .await
            {
                failed.push(
                    serde_json::json!({"repo": repo, "pr": pr_number, "error": format!("approve: {e}")}),
                );
                continue;
            }

            if let Err(e) = state
                .clients
                .forgejo
                .merge_pr(
                    owner,
                    repo,
                    pr_number,
                    forgejo_api::structs::MergePullRequestOptionDo::Squash,
                    true,
                )
                .await
            {
                failed.push(
                    serde_json::json!({"repo": repo, "pr": pr_number, "error": format!("merge: {e}")}),
                );
                continue;
            }

            let _ = state
                .clients
                .forgejo
                .remove_labels_by_name(owner, repo, pr_number, vec![QUEUED_LABEL.to_string()])
                .await;

            merged.push(serde_json::json!({"repo": repo, "pr": pr_number}));
        }
    }

    tracing::info!(
        merged = merged.len(),
        failed = failed.len(),
        ?merged,
        ?failed,
        "admin: merge-queued background run complete"
    );
}

#[derive(serde::Deserialize)]
pub struct AdminCommandRequest {
    pub owner: String,
    pub repo: String,
    pub number: i64,
    pub kind: AdminCommandKind,
    pub body: String,
}

#[derive(serde::Deserialize, Debug, Clone, Copy)]
#[serde(rename_all = "lowercase")]
pub enum AdminCommandKind {
    Pr,
    Issue,
}

pub async fn handle_admin_command(
    State(state): State<Arc<AppState>>,
    Json(req): Json<AdminCommandRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    tracing::info!(
        owner = req.owner,
        repo = req.repo,
        number = req.number,
        kind = ?req.kind,
        "admin: dispatching command"
    );

    match req.kind {
        AdminCommandKind::Pr => {
            let Some(parsed) = command::parse_pr_command(&req.body) else {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({"error": "unknown PR command"})),
                );
            };

            let cmd = event::CommentEvent {
                owner: req.owner,
                repo: req.repo,
                pr_number: req.number as u64,
                comment_id: 0,
                author: crate::FORGEJO_OWNER.to_string(),
                body: req.body.clone(),
            };

            let debug = format!("{parsed:?}");
            command::handle_pr_command(&state.clients, &state.orchestrator, &cmd, parsed).await;
            (StatusCode::OK, Json(serde_json::json!({"command": debug})))
        }
        AdminCommandKind::Issue => {
            let Some(parsed) = command::parse_issue_command(&req.body) else {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({"error": "unknown issue command"})),
                );
            };

            let issue = match state
                .clients
                .forgejo
                .get_issue(&req.owner, &req.repo, req.number)
                .await
            {
                Ok(i) => i,
                Err(e) => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(serde_json::json!({"error": format!("failed to fetch issue: {e}")})),
                    );
                }
            };

            let cmd = event::IssueCommentEvent {
                owner: req.owner,
                repo: req.repo,
                issue_number: req.number as u64,
                comment_id: 0,
                author: crate::FORGEJO_OWNER.to_string(),
                comment_body: req.body.clone(),
                issue_body: issue.body.unwrap_or_default(),
                issue_labels: issue
                    .labels
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|l| l.name)
                    .collect(),
            };

            let debug = format!("{parsed:?}");
            command::handle_issue_command(&state.clients, &cmd, parsed).await;
            (StatusCode::OK, Json(serde_json::json!({"command": debug})))
        }
    }
}

pub async fn handle_admin_argocd_resync(
    State(state): State<Arc<AppState>>,
    Path(app): Path<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    tracing::info!(app, "admin: triggering argocd resync");
    match state.clients.argocd.sync_application(&app).await {
        Ok(()) => (StatusCode::OK, Json(serde_json::json!({"synced": app}))),
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({"error": e.to_string()})),
        ),
    }
}

pub async fn handle_admin_metrics() -> (
    StatusCode,
    [(axum::http::header::HeaderName, &'static str); 1],
    String,
) {
    let body = janitor_bot::metrics::render();
    (
        StatusCode::OK,
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        body,
    )
}

pub async fn handle_admin_logs(
    State(state): State<Arc<AppState>>,
) -> (StatusCode, Json<serde_json::Value>) {
    let log = state.orchestrator.get_eval_log().await;
    (StatusCode::OK, Json(serde_json::to_value(&log).unwrap()))
}

pub async fn handle_admin_rules(
    State(state): State<Arc<AppState>>,
) -> (StatusCode, Json<serde_json::Value>) {
    let rules = state.orchestrator.rules_summary();
    (StatusCode::OK, Json(serde_json::to_value(&rules).unwrap()))
}

#[derive(Serialize)]
struct ServiceHealth {
    service: &'static str,
    healthy: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

pub async fn handle_admin_health_deep(
    State(state): State<Arc<AppState>>,
) -> (StatusCode, Json<serde_json::Value>) {
    let forgejo = match state.clients.forgejo.health_check().await {
        Ok(()) => ServiceHealth {
            service: "forgejo",
            healthy: true,
            error: None,
        },
        Err(e) => ServiceHealth {
            service: "forgejo",
            healthy: false,
            error: Some(e.to_string()),
        },
    };

    let github = match state.clients.github.health_check().await {
        Ok(()) => ServiceHealth {
            service: "github",
            healthy: true,
            error: None,
        },
        Err(e) => ServiceHealth {
            service: "github",
            healthy: false,
            error: Some(e.to_string()),
        },
    };

    let argocd = match state.clients.argocd.health_check().await {
        Ok(()) => ServiceHealth {
            service: "argocd",
            healthy: true,
            error: None,
        },
        Err(e) => ServiceHealth {
            service: "argocd",
            healthy: false,
            error: Some(e.to_string()),
        },
    };

    let services = [&forgejo, &github, &argocd];
    let all_healthy = services.iter().all(|s| s.healthy);

    let status = if all_healthy {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };

    (
        status,
        Json(serde_json::json!({
            "healthy": all_healthy,
            "services": [forgejo, github, argocd],
        })),
    )
}
