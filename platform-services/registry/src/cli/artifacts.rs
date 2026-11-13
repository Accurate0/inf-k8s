use axum::{
    Json,
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, header},
    response::{IntoResponse, Response},
};
use registry_api::{
    Artifact, ArtifactUrl, ECOSYSTEM_CLI, RegisterArtifact, SHA256_HEADER, Target,
    oci::{self, Manifest},
    package_repository,
};

use super::{db, release};
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

fn valid_digest(digest: &str) -> bool {
    digest
        .strip_prefix("sha256:")
        .is_some_and(|h| h.len() == 64 && h.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))
}

pub async fn register(
    State(state): State<AppState>,
    principal: Principal,
    Path((name, version, target)): Path<(String, String, String)>,
    Json(request): Json<RegisterArtifact>,
) -> Result<Json<Artifact>> {
    authorize_publish(&state, &principal, ECOSYSTEM_CLI, &name).await?;
    let target = parse_target(&target)?;
    let (Some(harbor), Some(config)) = (&state.harbor, &state.config.harbor) else {
        return Err(ApiError::Harbor(
            "this registry does not store artifacts in harbor".to_owned(),
        ));
    };
    if !valid_digest(&request.manifest) {
        return Err(ApiError::BadRequest(format!(
            "{:?} is not a sha256 manifest digest",
            request.manifest
        )));
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

    let repository = package_repository(&config.packages_project, &name);
    let fetched = harbor
        .oci()
        .manifest(&repository, &request.manifest)
        .await?
        .ok_or_else(|| {
            ApiError::Unprocessable(format!(
                "{repository}@{} is not in harbor; push it before registering",
                request.manifest
            ))
        })?;
    let manifest: Manifest = serde_json::from_slice(&fetched.body)
        .map_err(|e| ApiError::Unprocessable(format!("unreadable manifest: {e}")))?;
    let layer = release::validate_binary_manifest(
        &manifest,
        target,
        state.config.limits.max_artifact_bytes,
    )?;
    if !harbor.oci().blob_exists(&repository, &layer.digest).await? {
        return Err(ApiError::Unprocessable(format!(
            "{repository} is missing the {target} layer {}",
            layer.digest
        )));
    }
    let sha256 = layer.digest.trim_start_matches("sha256:").to_owned();
    let key = format!("{}{}", release::OCI_KEY_PREFIX, fetched.digest);

    sqlx::query!(
        "INSERT INTO cli_artifacts (version_id, target, sha256, size, object_key)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (version_id, target) DO UPDATE SET
            sha256 = EXCLUDED.sha256, size = EXCLUDED.size,
            object_key = EXCLUDED.object_key, uploaded_at = now()",
        row.id,
        target.as_str(),
        sha256,
        layer.size as i64,
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

    tracing::info!(package = name, version = row.version, %target, manifest = fetched.digest, "artifact registered");

    Ok(Json(Artifact {
        target,
        sha256,
        size: layer.size as i64,
    }))
}

const PRESIGN_SECS: u32 = 900;

async fn published_artifact(
    state: &AppState,
    name: &str,
    version: &str,
    target: Target,
) -> Result<(db::VersionRow, db::ArtifactRow)> {
    let package = db::find_package(&state.pool, name).await?;
    let row = db::get_version(&state.pool, &package, version).await?;
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
    Ok((row, artifact))
}

pub async fn download_url(
    State(state): State<AppState>,
    principal: Principal,
    Path((name, version, target)): Path<(String, String, String)>,
) -> Result<Json<ArtifactUrl>> {
    principal.require_reader()?;
    let target = parse_target(&target)?;
    let (_, artifact) = published_artifact(&state, &name, &version, target).await?;

    let presigned = if release::oci_manifest(&artifact.object_key).is_some() {
        None
    } else {
        state
            .storage
            .presign_get(&artifact.object_key, PRESIGN_SECS)
            .await?
    };
    let url = presigned.ok_or_else(|| {
        ApiError::NotFound(
            "this registry serves artifacts directly; download them from the artifact path"
                .to_owned(),
        )
    })?;
    metrics::record_download(ECOSYSTEM_CLI, &name, target.as_str());

    Ok(Json(ArtifactUrl {
        url,
        expires_at: chrono::Utc::now() + chrono::Duration::seconds(PRESIGN_SECS.into()),
        sha256: artifact.sha256,
        size: artifact.size,
    }))
}

pub async fn download(
    State(state): State<AppState>,
    principal: Principal,
    Path((name, version, target)): Path<(String, String, String)>,
) -> Result<Response> {
    principal.require_reader()?;
    let target = parse_target(&target)?;
    let (row, artifact) = published_artifact(&state, &name, &version, target).await?;

    let mut response = match (
        release::oci_manifest(&artifact.object_key),
        &state.harbor,
        &state.config.harbor,
    ) {
        (Some(_), Some(harbor), Some(config)) => {
            let blob = harbor
                .oci()
                .blob(
                    &package_repository(&config.packages_project, &name),
                    &oci::sha256_digest(&artifact.sha256),
                )
                .await?;
            Body::from_stream(blob.bytes_stream()).into_response()
        }
        (Some(_), _, _) => {
            return Err(ApiError::Harbor(
                "artifact is in harbor but harbor is not configured".to_owned(),
            ));
        }
        (None, _, _) => {
            Body::from_stream(state.storage.get_stream(&artifact.object_key).await?).into_response()
        }
    };
    metrics::record_download(ECOSYSTEM_CLI, &name, target.as_str());

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
