use axum::{
    extract::{Json, State},
    http::{HeaderMap, StatusCode},
};
use std::sync::Arc;
use tracing::Instrument;

use super::background_span;
use crate::AppState;
use janitor_bot::{command, event};

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
        if let Some(cmd) = event.into_issue_comment_event().filter(|_| comment_created)
            && cmd.author == crate::FORGEJO_OWNER
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
            && cmd.author == crate::FORGEJO_OWNER
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
