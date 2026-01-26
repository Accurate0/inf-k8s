use crate::{error::AppError, state::AppState};
use axum::{
    body::Bytes,
    extract::{Extension, Path, Query, State},
    response::Response,
};
use base64::{Engine, prelude::BASE64_STANDARD};
use serde_json::Value;

const BUCKET_NAME: &str = "object-registry-inf-k8s";

#[derive(serde::Serialize)]
struct ConfigResponseYaml {
    pub key: String,
    pub payload: serde_yaml::Value,
}

#[derive(serde::Deserialize)]
pub struct VersionQuery {
    pub version: Option<String>,
}

pub async fn put_object(
    State(state): State<AppState>,
    Extension(perms): Extension<crate::auth::Permissions>,
    Path((namespace, object)): Path<(String, String)>,
    Query(params): Query<VersionQuery>,
    body: Bytes,
) -> anyhow::Result<(), AppError> {
    state
        .permissions_manager
        .enforce(&perms, "PUT", &namespace)?;

    let key = match params.version {
        Some(v) => format!("{namespace}/{object}@{v}"),
        None => format!("{namespace}/{object}"),
    };

    state
        .s3_client
        .put_object()
        .bucket(BUCKET_NAME)
        .key(key)
        .body(body.into())
        .send()
        .await?;

    Ok(())
}

pub async fn get_object(
    State(state): State<AppState>,
    Extension(perms): Extension<crate::auth::Permissions>,
    Path((namespace, object)): Path<(String, String)>,
    Query(params): Query<VersionQuery>,
) -> Result<Response, AppError> {
    state
        .permissions_manager
        .enforce(&perms, "GET", &namespace)?;

    let key = match params.version {
        Some(v) => format!("{namespace}/{object}@{v}"),
        None => format!("{namespace}/{object}"),
    };
    let stored_object = state
        .s3_client
        .get_object()
        .key(&key)
        .bucket(BUCKET_NAME)
        .send()
        .await?;
    let object_value = stored_object.body.collect().await?;
    let bytes = object_value.to_vec();

    // detect content type
    let is_json_type = { serde_json::from_slice::<Value>(&bytes).is_ok() };
    let is_yaml_type = { serde_yaml::from_slice::<serde_yaml::Value>(&bytes).is_ok() };

    if is_json_type {
        Ok(Response::builder()
            .status(200)
            .header("Content-Type", "application/json")
            .body(
                serde_json::json!({ "key": key, "payload": serde_json::from_slice::<Value>(&bytes).unwrap()})
                    .to_string()
                    .into(),
            )?)
    } else if is_yaml_type {
        Ok(Response::builder()
            .status(200)
            .header("Content-Type", "application/yaml")
            .body(
                serde_yaml::to_string(&ConfigResponseYaml {
                    key,
                    payload: serde_yaml::from_slice::<serde_yaml::Value>(&bytes).unwrap(),
                })?
                .into(),
            )?)
    } else {
        Ok(Response::builder()
            .status(200)
            .header("Content-Type", "application/json")
            .body(
                serde_json::json!({ "key": key, "payload": BASE64_STANDARD.encode(bytes) })
                    .to_string()
                    .into(),
            )?)
    }
}
