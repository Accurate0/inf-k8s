use axum::{
    Json,
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, header},
    response::{IntoResponse, Response},
};
use registry_api::{Artifact, ECOSYSTEM_CLI, SHA256_HEADER, Target};

use super::db;
use crate::{
    auth::{Principal, policy::authorize_publish},
    error::{ApiError, Result},
    metrics,
    state::AppState,
    storage::cli_artifact_key,
};

fn parse_target(target: &str) -> Result<Target> {
    target.parse().map_err(ApiError::BadRequest)
}

pub async fn upload(
    State(state): State<AppState>,
    principal: Principal,
    Path((name, version, target)): Path<(String, String, String)>,
    headers: HeaderMap,
    body: Body,
) -> Result<Json<Artifact>> {
    authorize_publish(&state, &principal, ECOSYSTEM_CLI, &name).await?;
    let target = parse_target(&target)?;

    let expected = headers
        .get(SHA256_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::to_ascii_lowercase)
        .filter(|v| v.len() == 64 && v.chars().all(|c| c.is_ascii_hexdigit()))
        .ok_or_else(|| {
            ApiError::BadRequest(format!(
                "{SHA256_HEADER} header with a hex sha256 is required"
            ))
        })?;

    let max = state.config.limits.max_artifact_bytes;
    let declared_length = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    if declared_length.is_some_and(|len| len > max) {
        return Err(ApiError::TooLarge(max));
    }

    let package = db::find_package(&state.pool, &name).await?;
    let row = db::get_version(&state.pool, &package, &version).await?;
    if row.is_published() {
        return Err(ApiError::Conflict(
            "already_published",
            format!(
                "{name}@{} is published; its artifacts are immutable",
                row.version
            ),
        ));
    }
    if !row.targets().contains(&target) {
        return Err(ApiError::BadRequest(format!(
            "{name}@{} does not declare target {target}",
            row.version
        )));
    }

    let key = cli_artifact_key(&name, &row.version, target.as_str(), &row.binary_name);
    let stored = state
        .storage
        .put_stream(&key, body.into_data_stream(), max)
        .await?;

    if stored.sha256 != expected {
        state.storage.delete(&key).await?;
        return Err(ApiError::Unprocessable(format!(
            "sha256 mismatch: header says {expected}, body is {}",
            stored.sha256
        )));
    }

    sqlx::query!(
        "INSERT INTO cli_artifacts (version_id, target, sha256, size, object_key)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (version_id, target) DO UPDATE SET
            sha256 = EXCLUDED.sha256, size = EXCLUDED.size,
            object_key = EXCLUDED.object_key, uploaded_at = now()",
        row.id,
        target.as_str(),
        stored.sha256,
        stored.size as i64,
        key
    )
    .execute(&state.pool)
    .await
    .map_err(|e| match &e {
        sqlx::Error::Database(db) if db.code().as_deref() == Some("23514") => ApiError::Conflict(
            "already_published",
            format!("{name}@{} was published during the upload", row.version),
        ),
        _ => e.into(),
    })?;

    tracing::info!(package = name, version = row.version, %target, size = stored.size, "artifact uploaded");

    Ok(Json(Artifact {
        target,
        sha256: stored.sha256,
        size: stored.size as i64,
    }))
}

pub async fn download(
    State(state): State<AppState>,
    principal: Principal,
    Path((name, version, target)): Path<(String, String, String)>,
) -> Result<Response> {
    principal.require_reader()?;
    let target = parse_target(&target)?;

    let package = db::find_package(&state.pool, &name).await?;
    let row = db::get_version(&state.pool, &package, &version).await?;
    if !row.is_published() {
        return Err(ApiError::NotFound(format!(
            "{name}@{} is not published",
            row.version
        )));
    }

    let artifact = db::artifacts(&state.pool, row.id)
        .await?
        .into_iter()
        .find(|a| a.target == target.as_str())
        .ok_or_else(|| {
            ApiError::NotFound(format!("{name}@{} has no {target} build", row.version))
        })?;

    let stream = state.storage.get_stream(&artifact.object_key).await?;
    metrics::record_download(ECOSYSTEM_CLI, &name, target.as_str());

    let mut response = Body::from_stream(stream).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from(artifact.size));
    if let Ok(sha) = HeaderValue::from_str(&artifact.sha256) {
        headers.insert(SHA256_HEADER, sha);
    }
    if let Ok(disposition) =
        HeaderValue::from_str(&format!("attachment; filename=\"{}\"", row.binary_name))
    {
        headers.insert(header::CONTENT_DISPOSITION, disposition);
    }

    Ok(response)
}
