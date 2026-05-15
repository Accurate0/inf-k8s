use axum::{Json, Router, extract::State, http::StatusCode, routing::post};
use serde::Deserialize;
use std::sync::Arc;

use crate::event;
use crate::forgejo::ForgejoClient;
use crate::github::{self, GitHubClient};
use crate::rules::RulesOrchestrator;

pub struct AppState {
    pub client: ForgejoClient,
    pub github_client: GitHubClient,
    pub orchestrator: RulesOrchestrator,
}

#[derive(Deserialize)]
struct EvaluateRequest {
    r#type: String,
    payload: serde_json::Value,
}

async fn handle_evaluate(
    State(state): State<Arc<AppState>>,
    Json(request): Json<EvaluateRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
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
            let matched = state
                .orchestrator
                .explain_pr(&state.client, &mut pr_event)
                .await;
            state
                .orchestrator
                .evaluate_pr(&state.client, &mut pr_event)
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
            let matched = state
                .orchestrator
                .explain_workflow(&state.client, &state.github_client, &mut wf_event)
                .await;
            state
                .orchestrator
                .evaluate_workflow(&state.client, &state.github_client, &mut wf_event)
                .await;
            (
                StatusCode::OK,
                Json(serde_json::to_value(&matched).unwrap()),
            )
        }
        _ => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "type must be 'pr' or 'workflow'"})),
        ),
    }
}

pub fn test_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/evaluate", post(handle_evaluate))
        .with_state(state)
}
