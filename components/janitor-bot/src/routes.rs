use axum::{
    body::Bytes,
    extract::{Json, State},
    http::{HeaderMap, StatusCode},
};
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
