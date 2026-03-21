use crate::{error::AppError, state::AppState};
use axum::{
    body::Body,
    body::Bytes,
    extract::{Extension, Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::Response,
};
use object_registry_foundations::object_manager::ObjectManagerError;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

const XML_DECL: &str = r#"<?xml version="1.0" encoding="UTF-8"?>"#;
const S3_NS: &str = "http://s3.amazonaws.com/doc/2006-03-01/";

#[derive(Serialize)]
#[serde(rename = "ListBucketResult")]
struct ListBucketResult {
    #[serde(rename = "@xmlns")]
    xmlns: &'static str,
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "Prefix")]
    prefix: String,
    #[serde(rename = "MaxKeys")]
    max_keys: u32,
    #[serde(rename = "KeyCount", skip_serializing_if = "Option::is_none")]
    key_count: Option<usize>,
    #[serde(rename = "IsTruncated")]
    is_truncated: bool,
    #[serde(rename = "Contents", skip_serializing_if = "Vec::is_empty")]
    contents: Vec<S3Object>,
}

#[derive(Deserialize, Default)]
pub struct ListObjectsParams {
    #[serde(rename = "list-type")]
    list_type: Option<u8>,
    prefix: Option<String>,
    #[serde(rename = "max-keys")]
    max_keys: Option<u32>,
    #[allow(unused)]
    #[serde(rename = "continuation-token")]
    continuation_token: Option<String>,
}

#[derive(Serialize)]
struct S3Object {
    #[serde(rename = "Key")]
    key: String,
    #[serde(rename = "LastModified")]
    last_modified: String,
    #[serde(rename = "ETag")]
    etag: String,
    #[serde(rename = "Size")]
    size: usize,
    #[serde(rename = "StorageClass")]
    storage_class: &'static str,
}

#[derive(Serialize)]
#[serde(rename = "Error")]
struct S3ErrorBody {
    #[serde(rename = "Code")]
    code: &'static str,
    #[serde(rename = "Message")]
    message: &'static str,
}

fn to_xml<T: Serialize>(value: &T) -> Result<String, AppError> {
    let body = quick_xml::se::to_string(value)
        .map_err(|e| AppError::Error(anyhow::anyhow!("XML serialization error: {}", e)))?;
    Ok(format!("{XML_DECL}{body}"))
}

fn s3_error_response(status: StatusCode, code: &'static str, message: &'static str) -> Response {
    let xml = to_xml(&S3ErrorBody { code, message }).unwrap_or_else(|_| {
        format!("{XML_DECL}<Error><Code>{code}</Code><Message>{message}</Message></Error>")
    });
    Response::builder()
        .status(status)
        .header("Content-Type", "application/xml")
        .body(xml.into())
        .unwrap()
}

pub async fn list_objects(
    State(state): State<AppState>,
    Extension(perms): Extension<crate::auth::Permissions>,
    Path(bucket): Path<String>,
    Query(params): Query<ListObjectsParams>,
) -> Result<Response, AppError> {
    state
        .permissions_manager
        .enforce(&perms, "object:get", &bucket)?;

    let is_v2 = params.list_type == Some(2);
    let prefix = params.prefix.unwrap_or_default();
    let max_keys = params.max_keys.unwrap_or(1000);

    let mut objects = state.object_manager.list_objects(&bucket).await?;

    let effective_prefix = prefix.trim_start_matches('/');
    if !effective_prefix.is_empty() {
        objects.retain(|obj| obj.key.starts_with(effective_prefix));
    }

    let contents: Vec<S3Object> = objects
        .iter()
        .take(max_keys as usize)
        .map(|obj| S3Object {
            key: obj.key.clone(),
            last_modified: obj.metadata.created_at.clone(),
            etag: format!("\"{}\"", obj.metadata.checksum),
            size: obj.metadata.size,
            storage_class: "STANDARD",
        })
        .collect();

    let key_count = contents.len();

    let result = ListBucketResult {
        xmlns: S3_NS,
        name: bucket.clone(),
        prefix: effective_prefix.to_string(),
        max_keys,
        key_count: if is_v2 { Some(key_count) } else { None },
        is_truncated: objects.len() > max_keys as usize,
        contents,
    };

    let xml = to_xml(&result)?;

    let audit_id = state
        .audit_manager
        .log(
            "LIST_OBJECTS",
            &perms.issuer,
            Some(&bucket),
            None,
            HashMap::new(),
        )
        .await?;

    Ok(Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "application/xml")
        .header(object_registry::X_AUDIT_ID_HEADER, audit_id.to_string())
        .body(xml.into())?)
}

pub async fn put_object(
    State(state): State<AppState>,
    Extension(perms): Extension<crate::auth::Permissions>,
    headers: HeaderMap,
    Path((bucket, key)): Path<(String, String)>,
    body: Bytes,
) -> Result<Response, AppError> {
    state
        .permissions_manager
        .enforce(&perms, "object:put", &bucket)?;

    let content_type = headers
        .get("Content-Type")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("application/octet-stream");

    // S3 user metadata is passed via x-amz-meta-* headers
    let labels: HashMap<String, String> = headers
        .iter()
        .filter_map(|(k, v)| {
            let key_str = k.as_str();
            if key_str.starts_with("x-amz-meta-") {
                let label_key = key_str.trim_start_matches("x-amz-meta-").to_string();
                let label_value = v.to_str().unwrap_or("").to_string();
                Some((label_key, label_value))
            } else {
                None
            }
        })
        .collect();

    state
        .object_manager
        .put_object(
            &bucket,
            &key,
            body.to_vec(),
            content_type,
            &perms.issuer,
            labels.clone(),
        )
        .await?;

    let metadata = state.object_manager.get_metadata_for(&bucket, &key).await?;

    let mut details = labels;
    details.insert("content_type".to_string(), content_type.to_string());
    details.insert("size".to_string(), body.len().to_string());

    let audit_id = state
        .audit_manager
        .log(
            "PUT_OBJECT",
            &perms.issuer,
            Some(&bucket),
            Some(&key),
            details,
        )
        .await?;

    Ok(Response::builder()
        .status(StatusCode::OK)
        .header("ETag", format!("\"{}\"", metadata.checksum))
        .header(object_registry::X_AUDIT_ID_HEADER, audit_id.to_string())
        .body(Body::empty())?)
}

pub async fn get_object(
    State(state): State<AppState>,
    Extension(perms): Extension<crate::auth::Permissions>,
    Path((bucket, key)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    state
        .permissions_manager
        .enforce(&perms, "object:get", &bucket)?;

    // Support conditional GET via If-None-Match (strip quotes if present)
    let incoming_etag = headers
        .get("If-None-Match")
        .and_then(|h| h.to_str().ok())
        .map(|s| s.trim_matches('"').to_string());

    if let Some(etag) = &incoming_etag {
        let metadata = match state.object_manager.get_metadata_for(&bucket, &key).await {
            Ok(m) => m,
            Err(ObjectManagerError::ObjectNotFound) => {
                return Ok(s3_error_response(
                    StatusCode::NOT_FOUND,
                    "NoSuchKey",
                    "The specified key does not exist.",
                ));
            }
            Err(e) => return Err(e.into()),
        };

        if &metadata.checksum == etag {
            state
                .audit_manager
                .log("GET_OBJECT", &perms.issuer, Some(&bucket), Some(&key), {
                    let mut m = HashMap::new();
                    m.insert("etag_matched".to_string(), "true".to_string());
                    m
                })
                .await?;

            return Ok(Response::builder()
                .status(StatusCode::NOT_MODIFIED)
                .header("ETag", format!("\"{}\"", metadata.checksum))
                .body(Body::empty())?);
        }
    }

    let stored = match state.object_manager.get_object(&bucket, &key).await {
        Ok(o) => o,
        Err(ObjectManagerError::ObjectNotFound) => {
            return Ok(s3_error_response(
                StatusCode::NOT_FOUND,
                "NoSuchKey",
                "The specified key does not exist.",
            ));
        }
        Err(e) => return Err(e.into()),
    };

    let audit_id = state
        .audit_manager
        .log(
            "GET_OBJECT",
            &perms.issuer,
            Some(&bucket),
            Some(&key),
            HashMap::new(),
        )
        .await?;

    let etag = format!("\"{}\"", stored.metadata.checksum);
    let content_type = stored.metadata.content_type.clone();
    let last_modified = stored.metadata.created_at.clone();
    let size = stored.metadata.size;

    Ok(Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", content_type)
        .header("ETag", etag)
        .header("Last-Modified", last_modified)
        .header("Content-Length", size.to_string())
        .header(object_registry::X_AUDIT_ID_HEADER, audit_id.to_string())
        .body(stored.data.into())?)
}

pub async fn head_object(
    State(state): State<AppState>,
    Extension(perms): Extension<crate::auth::Permissions>,
    Path((bucket, key)): Path<(String, String)>,
) -> Result<Response, AppError> {
    state
        .permissions_manager
        .enforce(&perms, "object:get", &bucket)?;

    let metadata = match state.object_manager.get_metadata_for(&bucket, &key).await {
        Ok(m) => m,
        Err(ObjectManagerError::ObjectNotFound) => {
            return Ok(s3_error_response(
                StatusCode::NOT_FOUND,
                "NoSuchKey",
                "The specified key does not exist.",
            ));
        }
        Err(e) => return Err(e.into()),
    };

    Ok(Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", &metadata.content_type)
        .header("ETag", format!("\"{}\"", metadata.checksum))
        .header("Last-Modified", &metadata.created_at)
        .header("Content-Length", metadata.size.to_string())
        .body(Body::empty())?)
}

pub async fn delete_object(
    State(state): State<AppState>,
    Extension(perms): Extension<crate::auth::Permissions>,
    Path((bucket, key)): Path<(String, String)>,
) -> Result<Response, AppError> {
    state
        .permissions_manager
        .enforce(&perms, "object:delete", &bucket)?;

    state.object_manager.delete_object(&bucket, &key).await?;

    let audit_id = state
        .audit_manager
        .log(
            "DELETE_OBJECT",
            &perms.issuer,
            Some(&bucket),
            Some(&key),
            HashMap::new(),
        )
        .await?;

    Ok(Response::builder()
        .status(StatusCode::NO_CONTENT)
        .header(object_registry::X_AUDIT_ID_HEADER, audit_id.to_string())
        .body(Body::empty())?)
}
