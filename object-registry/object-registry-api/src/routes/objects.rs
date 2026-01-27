use crate::{error::AppError, state::AppState};
use aws_sdk_s3::operation::get_object::GetObjectError;
use axum::{
    body::Bytes,
    extract::{Extension, Path, Query, State},
    http::StatusCode,
    response::Response,
};
use base64::{Engine, prelude::BASE64_STANDARD};
use serde_json::Value;

pub const BUCKET_NAME: &str = "object-registry-inf-k8s";

#[derive(serde::Serialize)]
struct ConfigResponseYaml {
    pub key: String,
    pub payload: serde_yaml::Value,
}

#[derive(serde::Deserialize)]
pub struct VersionQuery {
    pub version: Option<String>,
}

fn validate_namespace(namespace: &str) -> Result<(), AppError> {
    if namespace == "public-keys" {
        return Err(AppError::Message(
            StatusCode::BAD_REQUEST,
            "Namespace 'public-keys' is reserved".to_string(),
        ));
    }
    Ok(())
}

pub async fn put_object(
    State(state): State<AppState>,
    Extension(perms): Extension<crate::auth::Permissions>,
    Path((namespace, object)): Path<(String, String)>,
    Query(params): Query<VersionQuery>,
    body: Bytes,
) -> anyhow::Result<(), AppError> {
    validate_namespace(&namespace)?;

    state
        .permissions_manager
        .enforce(&perms, "PUT", &namespace)?;

    let key = match params.version {
        Some(v) => format!("{namespace}/{object}@{v}"),
        None => format!("{namespace}/{object}"),
    };

    upload_object_to_s3(&state, key, body).await
}

pub async fn put_object_public(
    State(state): State<AppState>,
    Extension(perms): Extension<crate::auth::Permissions>,
    Path((namespace, object)): Path<(String, String)>,
    Query(params): Query<VersionQuery>,
    body: Bytes,
) -> anyhow::Result<(), AppError> {
    validate_namespace(&namespace)?;

    state
        .permissions_manager
        .enforce(&perms, "PUT", &namespace)?;

    let key = match params.version {
        Some(v) => format!("{namespace}/public/{object}@{v}"),
        None => format!("{namespace}/public/{object}"),
    };

    upload_object_to_s3(&state, key, body).await
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

    fetch_object_from_s3(&state, key).await
}

pub async fn get_object_public(
    State(state): State<AppState>,
    Path((namespace, object)): Path<(String, String)>,
    Query(params): Query<VersionQuery>,
) -> Result<Response, AppError> {
    let key = match params.version {
        Some(v) => format!("{namespace}/public/{object}@{v}"),
        None => format!("{namespace}/public/{object}"),
    };

    fetch_object_from_s3(&state, key).await
}

async fn upload_object_to_s3(
    state: &AppState,
    key: String,
    body: Bytes,
) -> anyhow::Result<(), AppError> {
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

async fn fetch_object_from_s3(state: &AppState, key: String) -> Result<Response, AppError> {
    let stored_object = match state
        .s3_client
        .get_object()
        .key(&key)
        .bucket(BUCKET_NAME)
        .send()
        .await
    {
        Ok(o) => o,
        Err(e) => {
            if let Some(svc) = e.as_service_error()
                && matches!(svc, GetObjectError::NoSuchKey(_))
            {
                return Err(AppError::StatusCode(StatusCode::NOT_FOUND));
            }
            return Err(e.into());
        }
    };
    let object_value = stored_object.body.collect().await?;
    let bytes = object_value.to_vec();

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
