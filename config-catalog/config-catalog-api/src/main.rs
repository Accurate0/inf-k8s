use axum::{
    Router,
    extract::Path,
    http::StatusCode,
    response::Json,
    routing::{get, put},
};
use lambda_http::{Error, run, tracing};
use serde_json::{Value, json};

async fn put_config(Path((namespace, object)): Path<(String, String)>) -> Json<Value> {
    Json(json!({ "msg": format!("{namespace} {object}") }))
}

async fn get_config(Path((namespace, object)): Path<(String, String)>) -> Json<Value> {
    Json(json!({ "msg": format!("{namespace} {object}") }))
}

async fn health_check() -> (StatusCode, String) {
    (StatusCode::OK, "OK".to_string())
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing::init_default_subscriber();

    let app = Router::new()
        .route("/{namespace}/{object}", put(put_config))
        .route("/{namespace}/{object}", get(get_config))
        .route("/health", get(health_check));

    run(app).await
}
