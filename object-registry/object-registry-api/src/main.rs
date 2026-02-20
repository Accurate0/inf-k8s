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
mod routes;
mod state;

async fn health_check() -> (StatusCode, String) {
    (StatusCode::OK, "OK".to_string())
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing::init_default_subscriber();

    let config = aws_config::load_from_env().await;

    let s3_client = aws_sdk_s3::Client::new(&config);
    let event_manager = object_registry_foundations::event_manager::EventManager::new(&config);
    let key_manager = object_registry_foundations::key_manager::KeyManager::new(&config);
    let object_manager = object_registry_foundations::object_manager::ObjectManager::new(&config);
    let audit_manager = object_registry_foundations::audit_manager::AuditManager::new(&config);
    let permissions_manager = permissions::PermissionsManager::new();

    let state = AppState {
        object_manager,
        s3_client,
        event_manager,
        key_manager,
        permissions_manager,
        audit_manager,
    };

    let app = Router::new()
        .route("/namespaces", get(routes::namespaces::list_namespaces))
        .route("/{namespace}", get(routes::objects::list_objects))
        .route("/{namespace}/{object}", put(routes::objects::put_object))
        .route("/{namespace}/{object}", get(routes::objects::get_object))
        .route(
            "/{namespace}/{object}",
            delete(routes::objects::delete_object),
        )
        .route("/audit", get(routes::audit::list_audit_logs))
        .route("/health", get(health_check))
        .route("/.well-known/jwks", get(routes::jwks::get_jwks))
        .route("/events/{namespace}", post(routes::events::post_event))
        .route("/events/{namespace}", get(routes::events::list_events))
        .route("/events/{namespace}/{id}", put(routes::events::put_event))
        .route(
            "/events/{namespace}/{id}",
            delete(routes::events::delete_event),
        )
        .with_state(state.clone())
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ));

    run(app).await
}
