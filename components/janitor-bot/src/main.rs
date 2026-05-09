mod event;
mod forgejo;
mod rules;

use axum::{
    Router, extract::Json, extract::State, http::HeaderMap, http::StatusCode, routing::post,
};
use forgejo::ForgejoClient;
use std::sync::Arc;

struct AppState {
    client: ForgejoClient,
    webhook_secret: String,
}

async fn handle_webhook(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(event): Json<event::WebhookEvent>,
) -> StatusCode {
    let auth = headers
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if auth != state.webhook_secret {
        return StatusCode::UNAUTHORIZED;
    }

    tracing::info!(action = event.action, "received webhook event");

    let Some(pr_event) = event.into_pr_event() else {
        return StatusCode::OK;
    };

    tokio::spawn(async move {
        let rules = rules::all_rules();
        rules::evaluate(&rules, &state.client, &pr_event).await;
    });

    StatusCode::OK
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let state = Arc::new(AppState {
        client: ForgejoClient::from_env()
            .expect("FORGEJO_INSTANCE_URL and FORGEJO_ACCESS_KEY must be set"),
        webhook_secret: std::env::var("FORGEJO_INCOMING_WEBHOOK_AUTH")
            .expect("FORGEJO_INCOMING_WEBHOOK_AUTH must be set"),
    });

    let app = Router::new()
        .route("/webhook", post(handle_webhook))
        .with_state(state);

    let addr = "0.0.0.0:3000";
    tracing::info!("listening on {addr}");
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
