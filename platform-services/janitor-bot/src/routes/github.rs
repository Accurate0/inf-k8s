use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
};
use std::sync::Arc;
use tracing::Instrument;

use super::{background_span, forward_headers};
use crate::AppState;
use janitor_bot::{event, github};

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
