use crate::{error::AppError, state::AppState};
use axum::{
    body::{Body, Bytes},
    extract::{Extension, Path, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::Response,
};
use base64::{Engine, prelude::BASE64_STANDARD};
use object_registry::types::{MetadataResponse, ObjectResponse};
use object_registry_foundations::object_manager::{
    ObjectManager, ObjectManagerError, ObjectMetadata,
};
use reqwest::header::{ETAG, IF_NONE_MATCH};
use serde_json::Value;
use std::collections::HashMap;

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
    body: Bytes,
) -> Result<Response, AppError> {
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
            body.to_vec(),
            content_type,
            &perms.issuer,
            labels.clone(),
        )
        .await?;

    let mut details = labels;
    details.insert("content_type".to_string(), content_type.to_string());
    details.insert("size".to_string(), body.len().to_string());

    let audit_id = state
        .audit_manager
        .log(
            "PUT_OBJECT",
            &perms.issuer,
            Some(&namespace),
            Some(&object),
            details,
        )
        .await?;

    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(object_registry::X_AUDIT_ID_HEADER, audit_id.to_string())
        .body(Body::empty())?)
}

pub async fn delete_object(
    State(state): State<AppState>,
    Extension(perms): Extension<crate::auth::Permissions>,
    Path((namespace, object)): Path<(String, String)>,
) -> Result<Response, AppError> {
    state
        .permissions_manager
        .enforce(&perms, "object:delete", &namespace)?;

    state
        .object_manager
        .delete_object(&namespace, &object)
        .await?;

    let audit_id = state
        .audit_manager
        .log(
            "DELETE_OBJECT",
            &perms.issuer,
            Some(&namespace),
            Some(&object),
            HashMap::new(),
        )
        .await?;

    Ok(Response::builder()
        .status(StatusCode::NO_CONTENT)
        .header(object_registry::X_AUDIT_ID_HEADER, audit_id.to_string())
        .body(Body::empty())?)
}

pub async fn get_object(
    State(state): State<AppState>,
    Extension(perms): Extension<crate::auth::Permissions>,
    Path((namespace, object)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let incoming_etag = headers
        .get(IF_NONE_MATCH)
        .map(|h| h.to_str().ok())
        .flatten();

    state
        .permissions_manager
        .enforce(&perms, "object:get", &namespace)?;

    let audit_id = state
        .audit_manager
        .log(
            "GET_OBJECT",
            &perms.issuer,
            Some(&namespace),
            Some(&object),
            HashMap::new(),
        )
        .await?;

    if let Some(etag) = incoming_etag {
        let metadata = state
            .object_manager
            .get_metadata_for(&namespace, &object)
            .await?;

        let checksum = metadata.checksum.to_owned();
        let mut response = if metadata.checksum == etag {
            Response::builder()
                .status(StatusCode::NOT_MODIFIED)
                .body(Body::empty())?
                .into()
        } else {
            let stored_object = match state
                .object_manager
                .get_object_only(&namespace, &object)
                .await
            {
                Ok(o) => o,
                Err(ObjectManagerError::ObjectNotFound) => {
                    return Err(AppError::StatusCode(StatusCode::NOT_FOUND));
                }
                Err(e) => return Err(e.into()),
            };

            let key = ObjectManager::get_key(&namespace, &object);

            convert_to_response(metadata, key, stored_object)?
        };

        let headers = response.headers_mut();

        headers.insert(
            object_registry::X_AUDIT_ID_HEADER,
            HeaderValue::from_str(&audit_id.to_string()).unwrap(),
        );
        headers.insert(ETAG, HeaderValue::from_str(&checksum).unwrap());

        Ok(response)
    } else {
        let (mut response, checksum) = fetch_object(&state, &namespace, &object).await?;
        let headers = response.headers_mut();

        headers.insert(
            object_registry::X_AUDIT_ID_HEADER,
            HeaderValue::from_str(&audit_id.to_string()).unwrap(),
        );

        headers.insert(ETAG, HeaderValue::from_str(&checksum).unwrap());

        Ok(response)
    }
}

pub async fn list_objects(
    State(state): State<AppState>,
    Extension(perms): Extension<crate::auth::Permissions>,
    Path(namespace): Path<String>,
) -> Result<Response, AppError> {
    state
        .permissions_manager
        .enforce(&perms, "object:get", &namespace)?;

    let audit_id = state
        .audit_manager
        .log(
            "LIST_OBJECTS",
            &perms.issuer,
            Some(&namespace),
            None,
            HashMap::new(),
        )
        .await?;

    let objects = state.object_manager.list_objects(&namespace).await?;

    let response = object_registry::types::ListObjectsResponse { objects };

    Ok(Response::builder()
        .status(200)
        .header("Content-Type", "application/json")
        .header(object_registry::X_AUDIT_ID_HEADER, audit_id.to_string())
        .body(serde_json::to_string(&response)?.into())?)
}

async fn fetch_object(
    state: &AppState,
    namespace: &str,
    object: &str,
) -> Result<(Response, String), AppError> {
    let stored_object = match state.object_manager.get_object(namespace, object).await {
        Ok(o) => o,
        Err(ObjectManagerError::ObjectNotFound) => {
            return Err(AppError::StatusCode(StatusCode::NOT_FOUND));
        }
        Err(e) => return Err(e.into()),
    };

    let key = stored_object.key;
    let bytes = stored_object.data;
    let checksum = stored_object.metadata.checksum.to_owned();
    let response = convert_to_response(stored_object.metadata, key, bytes)?;

    Ok((response, checksum))
}

fn convert_to_response(
    metadata: ObjectMetadata,
    key: String,
    bytes: Vec<u8>,
) -> Result<lambda_http::Response<Body>, AppError> {
    let meta = MetadataResponse {
        namespace: metadata.namespace,
        checksum: metadata.checksum.clone(),
        size: metadata.size,
        content_type: metadata.content_type,
        created_by: metadata.created_by,
        created_at: metadata.created_at,
        labels: metadata.labels,
    };
    let is_json_type = { serde_json::from_slice::<Value>(&bytes).is_ok() };
    let is_yaml_type = { serde_yaml::from_slice::<serde_yaml::Value>(&bytes).is_ok() };
    let response = if is_json_type {
        Response::builder()
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
            )?
    } else if is_yaml_type {
        Response::builder()
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
            )?
    } else {
        Response::builder()
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
            )?
    };
    Ok(response)
}
