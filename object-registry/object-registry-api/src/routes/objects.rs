use crate::{error::AppError, state::AppState};
use axum::{
    body::Bytes,
    extract::{Extension, Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::Response,
};
use base64::{Engine, prelude::BASE64_STANDARD};
use object_registry::object_manager::ObjectManagerError;
use object_registry::types::{MetadataResponse, ObjectResponse};
use serde_json::Value;
use std::collections::HashMap;

#[derive(serde::Deserialize)]
pub struct VersionQuery {
    pub version: Option<String>,
}

fn extract_labels(headers: &HeaderMap) -> HashMap<String, String> {
    headers
        .iter()
        .filter_map(|(k, v)| {
            let key_str = k.as_str();
            if key_str.starts_with("x-label-") {
                let label_key = key_str.trim_start_matches("x-label-").to_string();
                let label_value = v.to_str().unwrap_or("").to_string();
                Some((label_key, label_value))
            } else {
                None
            }
        })
        .collect()
}

pub async fn put_object(
    State(state): State<AppState>,
    Extension(perms): Extension<crate::auth::Permissions>,
    headers: HeaderMap,
    Path((namespace, object)): Path<(String, String)>,
    Query(params): Query<VersionQuery>,
    body: Bytes,
) -> anyhow::Result<(), AppError> {
    state
        .permissions_manager
        .enforce(&perms, "object:put", &namespace)?;

    let content_type = headers
        .get("Content-Type")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("application/octet-stream");

    let labels = extract_labels(&headers);

    state
        .object_manager
        .put_object(
            &namespace,
            &object,
            params.version.as_deref(),
            body.to_vec(),
            content_type,
            &perms.issuer,
            labels,
        )
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
        .enforce(&perms, "object:get", &namespace)?;

    fetch_object(&state, &namespace, &object, params.version.as_deref()).await
}

async fn fetch_object(
    state: &AppState,
    namespace: &str,
    object: &str,
    version: Option<&str>,
) -> Result<Response, AppError> {
    let stored_object = match state
        .object_manager
        .get_object(namespace, object, version)
        .await
    {
        Ok(o) => o,
        Err(ObjectManagerError::ObjectNotFound) => {
            return Err(AppError::StatusCode(StatusCode::NOT_FOUND));
        }
        Err(e) => return Err(e.into()),
    };

    let key = stored_object.key;
    let bytes = stored_object.data;
    let meta = MetadataResponse {
        namespace: stored_object.metadata.namespace,
        checksum: stored_object.metadata.checksum,
        size: stored_object.metadata.size,
        content_type: stored_object.metadata.content_type,
        created_by: stored_object.metadata.created_by,
        created_at: stored_object.metadata.created_at,
        version: stored_object.metadata.version,
        labels: stored_object.metadata.labels,
    };

    let is_json_type = { serde_json::from_slice::<Value>(&bytes).is_ok() };
    let is_yaml_type = { serde_yaml::from_slice::<serde_yaml::Value>(&bytes).is_ok() };

    if is_json_type {
        Ok(Response::builder()
            .status(200)
            .header("Content-Type", "application/json")
            .body(
                serde_json::to_string(&ObjectResponse {
                    is_base64_encoded: false,
                    key,
                    payload: serde_json::from_slice::<Value>(&bytes).unwrap(),
                    metadata: meta,
                })?
                .into(),
            )?)
    } else if is_yaml_type {
        Ok(Response::builder()
            .status(200)
            .header("Content-Type", "application/yaml")
            .body(
                serde_yaml::to_string(&ObjectResponse {
                    is_base64_encoded: false,
                    key,
                    payload: serde_yaml::from_slice::<serde_yaml::Value>(&bytes).unwrap(),
                    metadata: meta,
                })?
                .into(),
            )?)
    } else {
        Ok(Response::builder()
            .status(200)
            .header("Content-Type", "application/json")
            .body(
                serde_json::to_string(&ObjectResponse {
                    is_base64_encoded: true,
                    key,
                    payload: BASE64_STANDARD.encode(bytes),
                    metadata: meta,
                })?
                .into(),
            )?)
    }
}
