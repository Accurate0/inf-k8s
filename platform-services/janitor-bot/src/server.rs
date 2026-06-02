use axum::{Json, Router, extract::State, http::StatusCode, routing::post};
use serde::Deserialize;
use std::sync::Arc;

use crate::clients::Clients;
use crate::github;
use crate::rules::RulesOrchestrator;
use crate::{command, event};

pub struct AppState {
    pub clients: Clients,
    pub orchestrator: RulesOrchestrator,
}

#[derive(Deserialize)]
struct EvaluateRequest {
    r#type: String,
    payload: serde_json::Value,
    now: Option<chrono::DateTime<chrono::Utc>>,
}

async fn handle_evaluate(
    State(state): State<Arc<AppState>>,
    Json(request): Json<EvaluateRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    let override_orchestrator = request
        .now
        .map(|now| RulesOrchestrator::new().with_clock(std::sync::Arc::new(move || now)));

    let orchestrator = override_orchestrator
        .as_ref()
        .unwrap_or(&state.orchestrator);

    match request.r#type.as_str() {
        "pr" => {
            let webhook: event::WebhookEvent = match serde_json::from_value(request.payload) {
                Ok(w) => w,
                Err(e) => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(serde_json::json!({"error": e.to_string()})),
                    );
                }
            };
            let Some(mut pr_event) = webhook.into_pr_event() else {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({"error": "invalid PR payload"})),
                );
            };

            let matched = orchestrator
                .explain_and_evaluate_pr(&state.clients, &mut pr_event)
                .await;

            (
                StatusCode::OK,
                Json(serde_json::to_value(&matched).unwrap()),
            )
        }
        "workflow" => {
            let body = match serde_json::to_vec(&request.payload) {
                Ok(b) => b,
                Err(e) => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(serde_json::json!({"error": e.to_string()})),
                    );
                }
            };
            let Some(mut wf_event) = github::parse_workflow_event(&body) else {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({"error": "invalid workflow payload"})),
                );
            };

            let matched = orchestrator
                .explain_workflow(&state.clients, &mut wf_event)
                .await;
            orchestrator
                .evaluate_workflow(&state.clients, &mut wf_event)
                .await;

            (
                StatusCode::OK,
                Json(serde_json::to_value(&matched).unwrap()),
            )
        }
        "pr_comment" => {
            let webhook: event::WebhookEvent = match serde_json::from_value(request.payload) {
                Ok(w) => w,
                Err(e) => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(serde_json::json!({"error": e.to_string()})),
                    );
                }
            };
            let Some(cmd) = webhook.into_comment_event() else {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({"error": "invalid comment payload"})),
                );
            };

            let Some(parsed) = command::parse_pr_command(&cmd.body) else {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({"error": "unknown command"})),
                );
            };

            let debug = format!("{parsed:?}");
            command::handle_pr_command(&state.clients, &state.orchestrator, &cmd, parsed).await;

            (StatusCode::OK, Json(serde_json::json!({"command": debug})))
        }
        "commit_status" => {
            let body = match serde_json::to_vec(&request.payload) {
                Ok(b) => b,
                Err(e) => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(serde_json::json!({"error": e.to_string()})),
                    );
                }
            };
            let Some(cs_event) = github::parse_commit_status_event(&body) else {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({"error": "invalid commit status payload"})),
                );
            };

            let matched = orchestrator
                .explain_commit_status(&state.clients, &cs_event)
                .await;
            orchestrator
                .evaluate_commit_status(&state.clients, &cs_event)
                .await;

            (
                StatusCode::OK,
                Json(serde_json::to_value(&matched).unwrap()),
            )
        }
        "check_run" => {
            let body = match serde_json::to_vec(&request.payload) {
                Ok(b) => b,
                Err(e) => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(serde_json::json!({"error": e.to_string()})),
                    );
                }
            };
            let Some(mut cr_event) = github::parse_check_run_event(&body) else {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({"error": "invalid check_run payload"})),
                );
            };

            let matched = orchestrator
                .explain_check_run(&state.clients, &mut cr_event)
                .await;
            orchestrator
                .evaluate_check_run(&state.clients, &mut cr_event)
                .await;

            (
                StatusCode::OK,
                Json(serde_json::to_value(&matched).unwrap()),
            )
        }
        "push" => {
            let body = match serde_json::to_vec(&request.payload) {
                Ok(b) => b,
                Err(e) => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(serde_json::json!({"error": e.to_string()})),
                    );
                }
            };
            let Some(push_event) = github::parse_push_event(&body) else {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({"error": "invalid push payload"})),
                );
            };

            let raw = event::RawRequest {
                headers: vec![
                    ("content-type".into(), "application/json".into()),
                    ("x-github-event".into(), "push".into()),
                ],
                body,
            };

            let matched = orchestrator.explain_push(&state.clients, &push_event).await;
            orchestrator
                .evaluate_push(&state.clients, &push_event, &raw)
                .await;

            (
                StatusCode::OK,
                Json(serde_json::to_value(&matched).unwrap()),
            )
        }
        "argocd_sync" => {
            use crate::argocd::types::ArgoSyncPayload;
            let payload: ArgoSyncPayload = match serde_json::from_value(request.payload) {
                Ok(p) => p,
                Err(e) => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(serde_json::json!({"error": e.to_string()})),
                    );
                }
            };
            let sync_event = event::ArgoSyncEvent {
                app_name: payload.app_name,
                sha: payload.sha,
                sync_status: payload.sync_status,
                health_status: payload.health_status,
                phase: payload.phase,
                message: payload.message,
            };

            let matched = orchestrator
                .explain_argocd_sync(&state.clients, &sync_event)
                .await;
            orchestrator
                .evaluate_argocd_sync(&state.clients, &sync_event)
                .await;

            (
                StatusCode::OK,
                Json(serde_json::to_value(&matched).unwrap()),
            )
        }
        "issue_comment" => {
            let webhook: event::WebhookEvent = match serde_json::from_value(request.payload) {
                Ok(w) => w,
                Err(e) => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(serde_json::json!({"error": e.to_string()})),
                    );
                }
            };
            let Some(cmd) = webhook.into_issue_comment_event() else {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({"error": "invalid issue comment payload"})),
                );
            };

            let Some(parsed) = command::parse_issue_command(&cmd.comment_body) else {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({"error": "unknown command"})),
                );
            };

            command::handle_issue_command(&state.clients, &cmd, parsed).await;

            (
                StatusCode::OK,
                Json(serde_json::json!({"command": format!("{parsed:?}")})),
            )
        }
        _ => (
            StatusCode::BAD_REQUEST,
            Json(
                serde_json::json!({"error": "type must be 'pr', 'workflow', 'pr_comment', or 'issue_comment'"}),
            ),
        ),
    }
}

pub fn test_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/evaluate", post(handle_evaluate))
        .with_state(state)
}
