use axum::{extract::State, response::Html};
use std::sync::Arc;

use crate::AppState;
use janitor_bot::dashboard;

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
) -> Html<String> {
    let html = dashboard::render_dashboard(&state.clients, &state.orchestrator).await;
    state.cache.dashboard.insert((), Arc::from(html.as_str()));
    Html(html)
}
