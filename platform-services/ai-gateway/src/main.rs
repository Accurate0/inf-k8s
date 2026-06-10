use std::time::Duration;

use ai_gateway::{
    cache::CacheClient, config::Config, feature_flag::FeatureFlagClient, metrics,
    providers::Registry, routes, state::AppState, tracing_setup, usage,
};
use axum::{
    Router,
    http::StatusCode,
    routing::{delete, get, post},
};
use axum_tracing_opentelemetry::middleware::{OtelAxumLayer, OtelInResponseLayer};
use sqlx::postgres::PgPoolOptions;
use tokio::net::TcpListener;

/// How often `usage_daily` is refreshed from `usage_events`.
const ROLLUP_INTERVAL: Duration = Duration::from_secs(300);

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let tracer_provider = tracing_setup::init();
    metrics::init();

    let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let pool = PgPoolOptions::new()
        .max_connections(10)
        .connect(&database_url)
        .await?;
    tracing::info!("connected to postgres");

    sqlx::migrate!("./migrations").run(&pool).await?;
    tracing::info!("migrations applied");

    let config = Config::from_env();
    let providers = Registry::from_env();
    tracing::info!(providers = ?providers.names(), "loaded provider config");

    let features = FeatureFlagClient::from_env().await;
    let cache = CacheClient::from_env().await;
    if cache.is_some() {
        tracing::info!("dragonfly cache enabled");
    }

    let state = AppState::new(config, providers, pool.clone(), features, cache);

    // Background rollup so Grafana queries hit the small usage_daily table.
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(ROLLUP_INTERVAL);
        loop {
            ticker.tick().await;
            if let Err(e) = usage::refresh_rollup(&pool).await {
                tracing::error!("usage rollup failed: {e}");
            }
        }
    });

    let app = Router::new()
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
        .route("/admin/usage", get(routes::admin::usage_summary))
        .layer(OtelInResponseLayer)
        .layer(
            OtelAxumLayer::default().filter(|path| !matches!(path, "/health" | "/admin/metrics")),
        )
        .with_state(state);

    let port = std::env::var("PORT").unwrap_or_else(|_| "3000".to_string());
    let addr = format!("0.0.0.0:{port}");
    tracing::info!("listening on {addr}");
    let listener = TcpListener::bind(&addr).await?;

    axum::serve(listener, app).await?;

    if let Some(provider) = tracer_provider {
        let _ = provider.shutdown();
    }

    Ok(())
}
