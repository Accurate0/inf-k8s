use crate::{auth::auth_middleware, error::AppError, state::AppState};
use axum::{
    Router,
    body::Bytes,
    extract::{Path, State},
    http::StatusCode,
    middleware,
    response::Json,
    routing::{get, put},
};
use base64::{Engine, prelude::BASE64_STANDARD};
use lambda_http::{Error, run, tracing};
use serde_json::{Value, json};

mod auth;
mod error;
mod state;

const BUCKET_NAME: &str = "config-catalog-inf-k8s";

async fn put_config(
    State(AppState { s3_client, .. }): State<AppState>,
    Path((namespace, object)): Path<(String, String)>,
    body: Bytes,
) -> anyhow::Result<(), AppError> {
    let key = format!("{namespace}/{object}");

    s3_client
        .put_object()
        .bucket(BUCKET_NAME)
        .key(key)
        .body(body.into())
        .send()
        .await?;

    Ok(())
}

async fn get_config(
    State(AppState { s3_client, .. }): State<AppState>,
    Path((namespace, object)): Path<(String, String)>,
) -> Result<Json<Value>, AppError> {
    let key = format!("{namespace}/{object}");
    let stored_object = s3_client
        .get_object()
        .key(&key)
        .bucket(BUCKET_NAME)
        .send()
        .await?;
    let object_value = stored_object.body.collect().await?;
    let bytes = object_value.to_vec();

    let is_json_type = { serde_json::from_slice::<Value>(&bytes).is_ok() };
    if is_json_type {
        Ok(Json(json!({
            "key": key,
            "payload": serde_json::from_slice::<Value>(&bytes).unwrap()
        })))
    } else {
        Ok(Json(
            json!({ "key": key, "payload": BASE64_STANDARD.encode(bytes) }),
        ))
    }
}

async fn health_check() -> (StatusCode, String) {
    (StatusCode::OK, "OK".to_string())
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing::init_default_subscriber();

    let config = aws_config::load_from_env().await;
    let s3_client = aws_sdk_s3::Client::new(&config);
    let secrets_client = aws_sdk_secretsmanager::Client::new(&config);

    let state = AppState {
        s3_client,
        secrets_client,
    };

    let app = Router::new()
        .route("/{namespace}/{object}", put(put_config))
        .route("/{namespace}/{object}", get(get_config))
        .route("/health", get(health_check))
        .with_state(state.clone())
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ));

    run(app).await
}
