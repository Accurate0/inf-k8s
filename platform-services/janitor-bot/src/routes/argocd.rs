use axum::{
    extract::{Json, State},
    http::{HeaderMap, StatusCode},
};
use std::sync::Arc;
use tracing::Instrument;

use super::background_span;
use crate::AppState;
use janitor_bot::argocd::types::ArgoSyncPayload;
use janitor_bot::event;

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
