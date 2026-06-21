use axum::{
    Router,
    http::StatusCode,
    routing::{delete, get, post},
};
use axum_tracing_opentelemetry::middleware::{OtelAxumLayer, OtelInResponseLayer};

use crate::{routes, state::AppState};

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(|| async { StatusCode::OK }))
        .route("/v1/messages", post(routes::proxy::messages))
        .route("/v1/chat/completions", post(routes::proxy::chat_completions))
        .route("/v1/embeddings", post(routes::proxy::embeddings))
        .route("/v1/models", get(routes::admin::list_models))
        .route("/admin/metrics", get(routes::admin::metrics_handler))
        .route(
            "/admin/keys",
            post(routes::admin::create_key).get(routes::admin::list_keys),
        )
        .route(
            "/admin/keys/{id}",
            delete(routes::admin::revoke_key).patch(routes::admin::update_key),
        )
        .route(
            "/admin/keys/{id}/regenerate",
            post(routes::admin::regenerate_key),
        )
        .route("/admin/usage", get(routes::admin::usage_summary))
        .layer(OtelInResponseLayer)
        .layer(OtelAxumLayer::default().filter(|path| !matches!(path, "/health" | "/admin/metrics")))
        .with_state(state)
}
