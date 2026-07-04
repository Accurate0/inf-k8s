use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, header::CONTENT_TYPE},
    response::Html,
};
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use janitor_bot::dashboard;

#[derive(Debug, Default, Deserialize)]
pub struct DashboardRefresh {
    #[serde(default)]
    refetch: bool,
}

impl DashboardRefresh {
    fn parse(headers: &HeaderMap, body: &Bytes) -> Self {
        let is_json = headers
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|ct| ct.starts_with("application/json"));

        if is_json {
            serde_json::from_slice(body).unwrap_or_default()
        } else {
            serde_urlencoded::from_bytes(body).unwrap_or_default()
        }
    }
}

pub async fn handle_renovate_dashboard(State(state): State<Arc<AppState>>) -> Html<String> {
    if let Some(html) = state.cache.dashboard.get(&()) {
        return Html(html.to_string());
    }

    let html = dashboard::render_dashboard(&state.clients, &state.orchestrator).await;
    state.cache.dashboard.insert((), Arc::from(html.as_str()));
    Html(html)
}

pub async fn handle_renovate_dashboard_refresh(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Html<String> {
    let req = DashboardRefresh::parse(&headers, &body);
    if !req.refetch {
        return handle_renovate_dashboard(State(state)).await;
    }

    let html = dashboard::render_dashboard(&state.clients, &state.orchestrator).await;
    state.cache.dashboard.insert((), Arc::from(html.as_str()));
    Html(html)
}
