use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
};
use std::sync::Arc;
use tracing::Instrument;

use super::background_span;
use crate::AppState;
use janitor_bot::flux::{CLUSTER_ARTIFACT, FluxEvent};
use janitor_bot::github::verify_signature;

pub async fn handle_flux_webhook(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    let signature = headers
        .get("X-Signature")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if !verify_signature(&state.flux_webhook_secret, signature, &body) {
        return StatusCode::UNAUTHORIZED;
    }

    janitor_bot::metrics::record_webhook("flux");

    let event: FluxEvent = match serde_json::from_slice(&body) {
        Ok(event) => event,
        Err(e) => {
            tracing::warn!("invalid flux event: {e}");
            return StatusCode::BAD_REQUEST;
        }
    };

    let (Some(app), Some(commit)) = (event.app(), event.commit()) else {
        return StatusCode::OK;
    };

    tracing::info!(
        app,
        reason = event.reason,
        severity = event.severity,
        commit,
        "received flux event"
    );

    let sha = match state
        .clients
        .harbor
        .sync_commit(commit, CLUSTER_ARTIFACT)
        .await
    {
        Ok(sha) => sha,
        Err(e) => {
            tracing::warn!(app, "skipping flux event: {e:#}");
            return StatusCode::OK;
        }
    };

    let sync_event = event.to_sync(app, sha);
    tokio::spawn(
        async move {
            state
                .orchestrator
                .evaluate_sync(&state.clients, &sync_event)
                .await;
        }
        .instrument(background_span("flux_sync")),
    );

    StatusCode::OK
}
