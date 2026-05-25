use axum::{
    body::Bytes,
    extract::{Json, Path, State},
    http::{HeaderMap, StatusCode},
};
use serde::Serialize;
use std::sync::Arc;

use crate::AppState;
use janitor_bot::argocd::types::ArgoSyncPayload;
use janitor_bot::{command, event, github};

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

    if forgejo_event == "issue_comment" && (forgejo_event_type != "pull_request_comment") {
        if let Some(cmd) = event.into_issue_comment_event()
            && cmd.author == super::FORGEJO_OWNER
            && let Some(parsed) = command::parse_issue_command(&cmd.comment_body)
        {
            tokio::spawn(async move {
                command::handle_issue_command(&state.clients, &cmd, parsed).await;
            });
        }
        return StatusCode::OK;
    }

    if forgejo_event == "issue_comment" && forgejo_event_type == "pull_request_comment" {
        if let Some(cmd) = event.into_comment_event()
            && cmd.author == super::FORGEJO_OWNER
            && let Some(parsed) = command::parse_pr_command(&cmd.body)
        {
            tokio::spawn(async move {
                command::handle_pr_command(&state.clients, &state.orchestrator, &cmd, parsed).await;
            });
        }
        return StatusCode::OK;
    }

    if forgejo_event != "pull_request" {
        return StatusCode::OK;
    }

    let Some(mut pr_event) = event.into_pr_event() else {
        return StatusCode::OK;
    };

    tokio::spawn(async move {
        state
            .orchestrator
            .evaluate_pr(&state.clients, &mut pr_event)
            .await;
    });

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

        tokio::spawn(async move {
            state
                .orchestrator
                .evaluate_commit_status(&state.clients, &cs_event)
                .await;
        });
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

        tokio::spawn(async move {
            state
                .orchestrator
                .evaluate_check_run(&state.clients, &mut cr_event)
                .await;
        });
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

    tokio::spawn(async move {
        state
            .orchestrator
            .evaluate_workflow(&state.clients, &mut wf_event)
            .await;
    });

    StatusCode::OK
}

pub async fn handle_admin_cron(State(state): State<Arc<AppState>>) -> StatusCode {
    tracing::info!("admin: triggering cron evaluation");
    tokio::spawn(async move {
        super::evaluate_open_prs(&state).await;
    });
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

    tokio::spawn(async move {
        let pr = match state.clients.forgejo.get_pr(&owner, &repo, pr_number).await {
            Ok(pr) => pr,
            Err(e) => {
                tracing::error!(pr_number, "admin: failed to fetch PR: {e}");
                return;
            }
        };

        let Some(mut pr_event) = janitor_bot::event::PrEvent::from_api_pr(&pr, owner, repo) else {
            tracing::error!(pr_number, "admin: failed to convert PR to event");
            return;
        };

        state
            .orchestrator
            .evaluate_pr(&state.clients, &mut pr_event)
            .await;
    });

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
        }
    }

    (
        StatusCode::OK,
        Json(serde_json::json!({"merged": merged, "failed": failed})),
    )
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

    tokio::spawn(async move {
        state
            .orchestrator
            .evaluate_argocd_sync(&state.clients, &sync_event)
            .await;
    });

    StatusCode::OK
}
