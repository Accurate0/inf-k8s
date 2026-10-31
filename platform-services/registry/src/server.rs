use axum::{
    Router,
    http::{StatusCode, header},
    routing::get,
};
use axum_tracing_opentelemetry::middleware::{OtelAxumLayer, OtelInResponseLayer};

use crate::{cli, core, metrics, state::AppState};

const INSTALL_SCRIPT: &str = include_str!("install.sh");

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(|| async { StatusCode::OK }))
        .route("/metrics", get(|| async { metrics::render() }))
        .route(
            "/install.sh",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/x-shellscript")],
                    INSTALL_SCRIPT,
                )
            }),
        )
        .nest("/api/v1", core::router())
        .nest("/cli/v1", cli::router())
        .layer(OtelInResponseLayer)
        .layer(OtelAxumLayer::default().filter(|path| !matches!(path, "/health" | "/metrics")))
        .with_state(state)
}
