use crate::{auth::auth_middleware, state::AppState};
use axum::{
    Router,
    http::StatusCode,
    middleware,
    routing::{delete, get, post, put},
};
use lambda_http::{Error, run, tracing};

mod auth;
mod error;
mod permissions;
mod state;

// object bucket and response YAML type live in `routes::objects`

// `put_object` and `get_object` handlers moved to `routes::objects`.

async fn health_check() -> (StatusCode, String) {
    (StatusCode::OK, "OK".to_string())
}
// Event HTTP handlers live in `routes::events` to keep `main.rs` small.
// See `src/routes/events.rs` for implementations.
mod routes;

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing::init_default_subscriber();

    let config = aws_config::load_from_env().await;
    let s3_client = aws_sdk_s3::Client::new(&config);
    let secrets_client = aws_sdk_secretsmanager::Client::new(&config);
    let event_manager = object_registry::event_manager::EventManager::new(&config);
    let key_manager = object_registry::key_manager::KeyManager::new(&config);
    let permissions_manager = permissions::PermissionsManager::new();

    let state = AppState {
        s3_client,
        secrets_client,
        event_manager,
        key_manager,
        permissions_manager,
    };

    let app = Router::new()
        .route("/{namespace}/{object}", put(routes::objects::put_object))
        .route("/{namespace}/{object}", get(routes::objects::get_object))
        .route("/health", get(health_check))
        // Events
        .route("/events/:namespace", post(routes::events::post_event))
        .route("/events/:namespace", get(routes::events::list_events))
        .route("/events/:namespace/:id", put(routes::events::put_event))
        .route(
            "/events/:namespace/:id",
            delete(routes::events::delete_event),
        )
        .with_state(state.clone())
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ));

    run(app).await
}
