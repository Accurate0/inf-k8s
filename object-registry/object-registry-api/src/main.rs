use crate::{auth::auth_middleware, state::AppState};
use axum::{
    Router,
    http::StatusCode,
    middleware,
    routing::{delete, get, post, put},
};
use lambda_http::{Error, run, tracing};
use tower::ServiceExt;

mod auth;
mod error;
mod permissions;
mod routes;
mod s3_auth;
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
    let s3_key_manager = object_registry_foundations::s3_key_manager::S3KeyManager::new(&config);
    let object_manager = object_registry_foundations::object_manager::ObjectManager::new(&config);
    let audit_manager = object_registry_foundations::audit_manager::AuditManager::new(&config);
    let permissions_manager = permissions::PermissionsManager::new();

    let state = AppState {
        object_manager,
        s3_client,
        event_manager,
        key_manager,
        s3_key_manager,
        permissions_manager,
        audit_manager,
    };

    let s3_router = Router::new()
        .route("/{bucket}", get(routes::s3::list_objects))
        .route("/{bucket}/", get(routes::s3::list_objects))
        .route(
            "/{bucket}/{*key}",
            put(routes::s3::put_object)
                .get(routes::s3::get_object)
                .head(routes::s3::head_object)
                .delete(routes::s3::delete_object),
        )
        .with_state(state.clone())
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ));

    let api_router = Router::new()
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

    let app = tower::service_fn(move |req: lambda_http::Request| {
        let s3 = s3_router.clone();
        let api = api_router.clone();
        async move {
            let host = req
                .headers()
                .get("x-forwarded-host")
                .or_else(|| req.headers().get("host"))
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");

            tracing::info!("request: {:?}", req);
            if host.starts_with("s3.") {
                tracing::info!("s3 request");
                s3.oneshot(req).await
            } else {
                tracing::info!("api request");
                api.oneshot(req).await
            }
        }
    });

    run(app).await
}
