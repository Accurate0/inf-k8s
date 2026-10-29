use anyhow::Context;
use registry::{
    config::Config, jobs, metrics, server, state::AppState, storage::Storage, tracing_setup,
};
use sqlx::postgres::PgPoolOptions;
use tokio::net::TcpListener;

fn env_u32(name: &str, default: u32) -> u32 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let tracer_provider = tracing_setup::init();
    metrics::init();

    let database_url = std::env::var("DATABASE_URL").context("DATABASE_URL must be set")?;
    let min_connections = env_u32("DB_MIN_CONNECTIONS", 0);
    let max_connections = env_u32("DB_MAX_CONNECTIONS", 10);
    let pool = PgPoolOptions::new()
        .min_connections(min_connections)
        .max_connections(max_connections)
        .connect(&database_url)
        .await?;
    tracing::info!(min_connections, max_connections, "connected to postgres");

    sqlx::migrate!("./migrations").run(&pool).await?;
    tracing::info!("migrations applied");

    let config = Config::load()?;
    let storage = Storage::from_env(&config.storage)?;
    let state = AppState::new(config, pool, storage);

    state.auth.start().await;
    tokio::spawn(jobs::run(state.clone()));

    let app = server::router(state);

    let port = std::env::var("PORT").unwrap_or_else(|_| "3000".to_string());
    let addr = format!("0.0.0.0:{port}");
    tracing::info!("listening on {addr}");
    let listener = TcpListener::bind(&addr).await?;

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    tracing::info!("shutting down");

    if let Some(provider) = tracer_provider {
        let _ = provider.shutdown();
    }

    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl-C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }
}
