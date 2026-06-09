use axum::{
    body::Bytes,
    extract::{Json, Path, State},
    http::{HeaderMap, StatusCode},
};
use opentelemetry::trace::TraceContextExt;
use serde::Serialize;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;
use tracing::{Instrument, Span};
use tracing_opentelemetry::OpenTelemetrySpanExt;

use crate::AppState;
use janitor_bot::argocd::types::ArgoSyncPayload;
use janitor_bot::{command, event, github};

/// Root span for background work spawned from a webhook handler.
///
/// Detached from the current request span (`parent: None`) so the request span
/// can close as soon as the HTTP handler returns. We add a span link back to
/// the originating request so the two traces remain navigable in Tempo without
/// the active-span leak that lets unrelated incoming requests reparent under
/// the webhook trace.
fn background_span(name: &'static str) -> Span {
    let span = tracing::info_span!(parent: None, "background", task = name, otel.name = format!("background: {name}"));

    let request_ctx = Span::current().context();
    let request_span = request_ctx.span();
    let request_sc = request_span.span_context();
    if request_sc.is_valid() {
        span.add_link(request_sc.clone());

        let bg_sc = span.context().span().span_context().clone();
        if bg_sc.is_valid() {
            Span::current().add_link(bg_sc);
        }
    }
    span
}

/// Headers worth forwarding when proxying a webhook downstream. Notably the
/// GitHub event type and signature, so the downstream service can re-validate
/// the payload against its own copy of the shared secret.
const FORWARD_HEADERS: &[&str] = &[
    "content-type",
    "user-agent",
    "x-github-event",
    "x-github-delivery",
    "x-hub-signature",
    "x-hub-signature-256",
];

fn forward_headers(headers: &HeaderMap) -> Vec<(String, String)> {
    FORWARD_HEADERS
        .iter()
        .filter_map(|name| {
            headers
                .get(*name)
                .and_then(|v| v.to_str().ok())
                .map(|v| ((*name).to_string(), v.to_string()))
        })
        .collect()
}

pub async fn handle_forgejo_webhook(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(event): Json<event::WebhookEvent>,
) -> StatusCode {
    let auth = headers
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if auth != state.forgejo_webhook_secret {
        return StatusCode::UNAUTHORIZED;
    }

    let forgejo_event = headers
        .get("X-Forgejo-Event")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let forgejo_event_type = headers
        .get("X-Forgejo-Event-Type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    janitor_bot::metrics::record_webhook("forgejo");

    tracing::info!(
        action = event.action,
        forgejo_event,
        forgejo_event_type,
        "received forgejo webhook event"
    );

    // Deleting (or otherwise mutating) a comment re-delivers an `issue_comment`
    // webhook whose payload still carries the original comment body. Only act on
    // newly created comments so removing a `janitor ...` command doesn't re-run it.
    let comment_created = event.action == "created";

    if forgejo_event == "issue_comment" && (forgejo_event_type != "pull_request_comment") {
        if let Some(cmd) = event.into_issue_comment_event()
            .filter(|_| comment_created)
            && cmd.author == super::FORGEJO_OWNER
            && let Some(parsed) = command::parse_issue_command(&cmd.comment_body)
        {
            tokio::spawn(
                async move {
                    command::handle_issue_command(&state.clients, &cmd, parsed).await;
                }
                .instrument(background_span("issue_command")),
            );
        }
        return StatusCode::OK;
    }

    if forgejo_event == "issue_comment" && forgejo_event_type == "pull_request_comment" {
        if let Some(cmd) = event.into_comment_event().filter(|_| comment_created)
            && cmd.author == super::FORGEJO_OWNER
            && let Some(parsed) = command::parse_pr_command(&cmd.body)
        {
            tokio::spawn(
                async move {
                    command::handle_pr_command(&state.clients, &state.orchestrator, &cmd, parsed)
                        .await;
                }
                .instrument(background_span("pr_command")),
            );
        }
        return StatusCode::OK;
    }

    if forgejo_event != "pull_request" {
        return StatusCode::OK;
    }

    let Some(mut pr_event) = event.into_pr_event() else {
        return StatusCode::OK;
    };

    tokio::spawn(
        async move {
            state
                .orchestrator
                .evaluate_pr(&state.clients, &mut pr_event)
                .await;
        }
        .instrument(background_span("forgejo_pr_event")),
    );

    StatusCode::OK
}

pub async fn handle_github_webhook(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    let signature = headers
        .get("X-Hub-Signature-256")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if !github::verify_signature(&state.github_webhook_secret, signature, &body) {
        return StatusCode::UNAUTHORIZED;
    }

    janitor_bot::metrics::record_webhook("github");

    let github_event = headers
        .get("X-GitHub-Event")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if github_event == "status" {
        let Some(cs_event) = github::parse_commit_status_event(&body) else {
            tracing::info!("received github status event but failed to parse");
            return StatusCode::OK;
        };

        tracing::info!(
            repository = cs_event.repository,
            context = cs_event.context,
            state = cs_event.state,
            "received github commit status event"
        );

        tokio::spawn(
            async move {
                state
                    .orchestrator
                    .evaluate_commit_status(&state.clients, &cs_event)
                    .await;
            }
            .instrument(background_span("github_status")),
        );
        return StatusCode::OK;
    }

    if github_event == "check_run" {
        let Some(mut cr_event) = github::parse_check_run_event(&body) else {
            tracing::info!(
                "received github check_run event but not actionable (not completed or failed to parse)"
            );
            return StatusCode::OK;
        };

        tracing::info!(
            repository = cr_event.repository,
            name = cr_event.name,
            conclusion = cr_event.conclusion,
            "received github check_run event"
        );

        tokio::spawn(
            async move {
                state
                    .orchestrator
                    .evaluate_check_run(&state.clients, &mut cr_event)
                    .await;
            }
            .instrument(background_span("github_status")),
        );
        return StatusCode::OK;
    }

    if github_event == "push" {
        let Some(push_event) = github::parse_push_event(&body) else {
            tracing::info!("received github push event but failed to parse");
            return StatusCode::OK;
        };

        tracing::info!(
            repository = push_event.repository,
            branch = push_event.branch,
            "received github push event"
        );

        // Capture the raw request so the `proxy_pass` action can forward it
        // verbatim (preserving the signature) to a downstream service.
        let raw = event::RawRequest {
            body: body.to_vec(),
            headers: forward_headers(&headers),
        };

        tokio::spawn(
            async move {
                state
                    .orchestrator
                    .evaluate_push(&state.clients, &push_event, &raw)
                    .await;
            }
            .instrument(background_span("github_push")),
        );
        return StatusCode::OK;
    }

    let Some(mut wf_event) = github::parse_workflow_event(&body) else {
        tracing::info!("received github webhook (not a workflow_run event, ignoring)");
        return StatusCode::OK;
    };

    tracing::info!(
        workflow = wf_event.workflow_name,
        conclusion = wf_event.conclusion,
        "received github workflow event"
    );

    tokio::spawn(
        async move {
            state
                .orchestrator
                .evaluate_workflow(&state.clients, &mut wf_event)
                .await;
        }
        .instrument(background_span("github_workflow")),
    );

    StatusCode::OK
}

pub async fn handle_admin_cron(State(state): State<Arc<AppState>>) -> StatusCode {
    tracing::info!("admin: triggering cron evaluation");
    tokio::spawn(
        async move {
            super::evaluate_open_prs(&state).await;
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

    for repo in super::WATCH_REPOS {
        let prs = match state
            .clients
            .forgejo
            .list_open_prs(super::FORGEJO_OWNER, repo)
            .await
        {
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
                .is_pr_approved_by_bot(super::FORGEJO_OWNER, repo, pr_number)
                .await
                && let Err(e) = state
                    .clients
                    .forgejo
                    .approve_pr(
                        super::FORGEJO_OWNER,
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
                    super::FORGEJO_OWNER,
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
                .remove_labels_by_name(
                    super::FORGEJO_OWNER,
                    repo,
                    pr_number,
                    vec![QUEUED_LABEL.to_string()],
                )
                .await;

            merged.push(serde_json::json!({"repo": repo, "pr": pr_number}));

            // Forgejo needs a moment to recompute mergeability for the next queued PR
            // after a merge changes the target branch head.
            sleep(Duration::from_secs(5)).await;
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
                author: super::FORGEJO_OWNER.to_string(),
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
                author: super::FORGEJO_OWNER.to_string(),
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

pub async fn handle_argocd_webhook(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(payload): Json<ArgoSyncPayload>,
) -> StatusCode {
    let auth = headers
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if auth != state.argocd_webhook_secret {
        return StatusCode::UNAUTHORIZED;
    }

    janitor_bot::metrics::record_webhook("argocd");

    tracing::info!(
        app_name = payload.app_name,
        phase = payload.phase,
        health = payload.health_status,
        sha = payload.sha,
        "received argocd sync event"
    );

    if payload.sha.is_empty() {
        tracing::info!(
            app_name = payload.app_name,
            "skipping argocd sync event with empty sha"
        );
        return StatusCode::OK;
    }

    let sync_event = event::ArgoSyncEvent {
        app_name: payload.app_name,
        sha: payload.sha,
        sync_status: payload.sync_status,
        health_status: payload.health_status,
        phase: payload.phase,
        message: payload.message,
    };

    tokio::spawn(
        async move {
            state
                .orchestrator
                .evaluate_argocd_sync(&state.clients, &sync_event)
                .await;
        }
        .instrument(background_span("argocd_sync")),
    );

    StatusCode::OK
}
