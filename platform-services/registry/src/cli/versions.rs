use std::collections::BTreeSet;

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use chrono::Utc;
use registry_api::{
    Artifact, CreateVersion, ECOSYSTEM_CLI, Target, Version, Yank,
    signing::{self, ManifestClaims, PublishRequest, Signatures, Statement},
};
use serde_json::json;

use super::{
    db::{self, DRAFT},
    packages::{validate_aliases, validate_binary_name},
};
use crate::{
    auth::{Principal, policy::authorize_publish},
    core::audit::{self, Event},
    error::{ApiError, Result},
    metrics,
    state::AppState,
};

pub async fn get(
    State(state): State<AppState>,
    principal: Principal,
    Path((name, version)): Path<(String, String)>,
) -> Result<Json<Version>> {
    principal.require_reader()?;

    let package = db::find_package(&state.pool, &name).await?;
    let version = db::get_version(&state.pool, &package, &version).await?;
    let artifacts = db::artifacts(&state.pool, version.id).await?;
    Ok(Json(version.into_api(&name, &artifacts)))
}

pub async fn create(
    State(state): State<AppState>,
    principal: Principal,
    Path(name): Path<String>,
    Json(request): Json<CreateVersion>,
) -> Result<(StatusCode, Json<Version>)> {
    authorize_publish(&state, &principal, ECOSYSTEM_CLI, &name).await?;

    let package = db::find_package(&state.pool, &name).await?;
    let version = db::parse_version(&request.version)?.to_string();

    let targets: BTreeSet<Target> = request.targets.iter().copied().collect();
    if targets.is_empty() {
        return Err(ApiError::BadRequest(
            "a version needs at least one target".to_owned(),
        ));
    }
    let targets: Vec<String> = targets.iter().map(|t| t.as_str().to_owned()).collect();

    let aliases = request
        .aliases
        .clone()
        .unwrap_or_else(|| package.aliases.clone());
    validate_aliases(&aliases)?;
    let description = request
        .description
        .clone()
        .unwrap_or_else(|| package.description.clone());
    let binary_name = request
        .binary_name
        .clone()
        .unwrap_or_else(|| package.binary_name.clone());
    validate_binary_name(&binary_name)?;
    if let Some(auth) = &request.auth {
        auth.validate().map_err(ApiError::BadRequest)?;
    }
    let auth = request.auth.clone().map(sqlx::types::Json);

    let existing = db::find_version(&state.pool, package.id, &version).await?;
    let status = match &existing {
        Some(v) if v.is_published() => {
            return Err(ApiError::Conflict(
                "already_published",
                format!("{name}@{version} is already published"),
            ));
        }
        Some(_) => StatusCode::OK,
        None => StatusCode::CREATED,
    };

    sqlx::query!(
        "INSERT INTO cli_versions (package_id, version, targets, description, binary_name, aliases, auth)
         VALUES ($1, $2, $3, $4, $5, $6, $7)
         ON CONFLICT (package_id, version) DO UPDATE SET
            targets = EXCLUDED.targets,
            description = EXCLUDED.description,
            binary_name = EXCLUDED.binary_name,
            aliases = EXCLUDED.aliases,
            auth = EXCLUDED.auth
         WHERE cli_versions.status = 'draft'",
        package.id,
        version,
        &targets,
        description,
        binary_name,
        &aliases,
        auth as Option<sqlx::types::Json<registry_api::PackageAuth>>
    )
    .execute(&state.pool)
    .await?;

    let row = db::get_version(&state.pool, &package, &version).await?;
    let artifacts = db::artifacts(&state.pool, row.id).await?;
    Ok((status, Json(row.into_api(&name, &artifacts))))
}

pub async fn publish(
    State(state): State<AppState>,
    principal: Principal,
    Path((name, version)): Path<(String, String)>,
    Json(request): Json<PublishRequest>,
) -> Result<Json<Version>> {
    let publisher = authorize_publish(&state, &principal, ECOSYSTEM_CLI, &name).await?;

    let package = db::find_package(&state.pool, &name).await?;
    let row = db::get_version(&state.pool, &package, &version).await?;
    if row.is_published() {
        return Err(ApiError::Conflict(
            "already_published",
            format!("{name}@{} is already published", row.version),
        ));
    }

    let artifacts = db::artifacts(&state.pool, row.id).await?;
    let missing: Vec<&String> = row
        .targets
        .iter()
        .filter(|t| !artifacts.iter().any(|a| &a.target == *t))
        .collect();
    if !missing.is_empty() {
        return Err(ApiError::Unprocessable(format!(
            "{name}@{} is missing artifacts for: {}",
            row.version,
            missing
                .iter()
                .map(|t| t.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }

    let roots = [state.signer.verifying_key()];
    let (certificate, key) =
        signing::verify_certificate(&roots, &request.certificate, ECOSYSTEM_CLI, &name)
            .map_err(|e| ApiError::BadRequest(format!("signing certificate: {e}")))?;
    let published_at = Utc::now();
    if published_at > certificate.expires_at {
        return Err(ApiError::BadRequest(
            "signing certificate has expired".to_owned(),
        ));
    }

    let mut statement: Statement = signing::open(&request.statement, &key)
        .map_err(|e| ApiError::BadRequest(format!("signed statement: {e}")))?;
    statement.artifacts.sort_by_key(|a| a.target);
    if let Some(permissions) = &statement.permissions {
        permissions.validate().map_err(ApiError::BadRequest)?;
    }
    let mut uploaded: Vec<Artifact> = artifacts
        .iter()
        .filter_map(|a| {
            Some(Artifact {
                target: a.target.parse().ok()?,
                sha256: a.sha256.clone(),
                size: a.size,
            })
        })
        .collect();
    uploaded.sort_by_key(|a| a.target);
    if statement.package != name
        || statement.version != row.version
        || statement.artifacts != uploaded
    {
        return Err(ApiError::Unprocessable(
            "signed statement does not match the uploaded artifacts".to_owned(),
        ));
    }

    let manifest = signing::sign(
        state.signer.key(),
        &ManifestClaims {
            statement,
            signed_by: certificate.subject,
            published_by: publisher.actor.clone(),
            source: publisher.source.clone(),
            published_at,
        },
    );
    let signatures = sqlx::types::Json(Signatures {
        manifest,
        certificate: request.certificate,
        statement: request.statement,
    });

    let source = publisher.source.clone().unwrap_or_default();
    let updated = sqlx::query!(
        "UPDATE cli_versions SET
            status = 'published', published_at = $9, published_by = $2, publisher_kind = $3,
            source_repository = $4, source_ref = $5, source_sha = $6, source_run_id = $7,
            signatures = $10
         WHERE id = $1 AND status = $8",
        row.id,
        publisher.actor,
        publisher.kind,
        source.repository,
        source.git_ref,
        source.sha,
        source.run_id,
        DRAFT,
        published_at,
        signatures as sqlx::types::Json<Signatures>
    )
    .execute(&state.pool)
    .await?
    .rows_affected();

    if updated == 0 {
        return Err(ApiError::Conflict(
            "already_published",
            format!("{name}@{} was published concurrently", row.version),
        ));
    }

    let versions = db::versions(&state.pool, package.id).await?;
    if !row.description.is_empty()
        && db::latest(&versions, None, false).is_some_and(|v| v.id == row.id)
    {
        sqlx::query!(
            "UPDATE cli_packages SET description = $2, updated_at = now() WHERE id = $1",
            package.id,
            row.description
        )
        .execute(&state.pool)
        .await?;
    }

    audit::record(
        &state.pool,
        &publisher.actor,
        publisher.kind,
        Event {
            action: "version.publish",
            ecosystem: Some(ECOSYSTEM_CLI),
            package: Some(&name),
            version: Some(&row.version),
            detail: json!({
                "targets": row.targets,
                "source": publisher.source,
            }),
        },
    )
    .await?;
    metrics::record_publish(ECOSYSTEM_CLI, &name, publisher.kind);
    tracing::info!(
        package = name,
        version = row.version,
        actor = publisher.actor,
        "published"
    );

    let row = db::get_version(&state.pool, &package, &row.version).await?;
    Ok(Json(row.into_api(&name, &artifacts)))
}

pub async fn yank(
    State(state): State<AppState>,
    principal: Principal,
    Path((name, version)): Path<(String, String)>,
    body: Option<Json<Yank>>,
) -> Result<Json<Version>> {
    let publisher = authorize_publish(&state, &principal, ECOSYSTEM_CLI, &name).await?;
    let reason = body.and_then(|Json(y)| y.reason);
    set_yanked(
        &state,
        &name,
        &version,
        Some((&publisher.actor, reason)),
        &principal,
    )
    .await
}

pub async fn unyank(
    State(state): State<AppState>,
    principal: Principal,
    Path((name, version)): Path<(String, String)>,
) -> Result<Json<Version>> {
    principal.require_admin()?;
    set_yanked(&state, &name, &version, None, &principal).await
}

async fn set_yanked(
    state: &AppState,
    name: &str,
    version: &str,
    yank: Option<(&str, Option<String>)>,
    principal: &Principal,
) -> Result<Json<Version>> {
    let package = db::find_package(&state.pool, name).await?;
    let row = db::get_version(&state.pool, &package, version).await?;
    if !row.is_published() {
        return Err(ApiError::Conflict(
            "not_published",
            format!("{name}@{} is not published", row.version),
        ));
    }

    let (by, reason) = match &yank {
        Some((by, reason)) => (Some(*by), reason.clone()),
        None => (None, None),
    };

    sqlx::query!(
        "UPDATE cli_versions SET
            yanked_at = CASE WHEN $2::text IS NULL THEN NULL ELSE now() END,
            yanked_by = $2, yank_reason = $3
         WHERE id = $1",
        row.id,
        by,
        reason
    )
    .execute(&state.pool)
    .await?;

    audit::record_as(
        &state.pool,
        principal,
        Event {
            action: if yank.is_some() {
                "version.yank"
            } else {
                "version.unyank"
            },
            ecosystem: Some(ECOSYSTEM_CLI),
            package: Some(name),
            version: Some(&row.version),
            detail: json!({ "reason": reason }),
        },
    )
    .await?;

    let row = db::get_version(&state.pool, &package, &row.version).await?;
    let artifacts = db::artifacts(&state.pool, row.id).await?;
    Ok(Json(row.into_api(name, &artifacts)))
}
