use std::collections::HashMap;

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use registry_api::{
    CreatePackage, ECOSYSTEM_CLI, Package, PackageDetail, Target, UpdatePackage, Version,
    validate_package_name,
};
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use super::db::{self, PackageRow, VersionRow};
use crate::{
    auth::Principal,
    core::{
        audit::{self, Event},
        publishers,
    },
    error::{ApiError, Result},
    state::AppState,
};

fn to_api(row: PackageRow, latest: Option<&VersionRow>) -> Package {
    Package {
        name: row.name,
        description: row.description,
        binary_name: row.binary_name,
        aliases: row.aliases,
        latest: latest.map(|v| v.version.clone()),
        created_at: row.created_at,
        archived: row.archived_at.is_some(),
    }
}

pub fn validate_aliases(aliases: &[String]) -> Result<()> {
    for alias in aliases {
        validate_package_name(alias).map_err(ApiError::BadRequest)?;
    }
    Ok(())
}

pub fn validate_binary_name(binary: &str) -> Result<()> {
    registry_api::validate_binary_name(binary).map_err(ApiError::BadRequest)
}

#[derive(Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    q: Option<String>,
}

pub async fn list(
    State(state): State<AppState>,
    principal: Principal,
    Query(query): Query<ListQuery>,
) -> Result<Json<Vec<Package>>> {
    principal.require_reader()?;

    let packages =
        db::all_packages(&state.pool, query.q.as_deref().filter(|q| !q.is_empty())).await?;
    let mut versions: HashMap<Uuid, Vec<VersionRow>> = HashMap::new();
    for version in db::published_versions(&state.pool).await? {
        versions
            .entry(version.package_id)
            .or_default()
            .push(version);
    }

    Ok(Json(
        packages
            .into_iter()
            .map(|p| {
                let latest = versions
                    .get(&p.id)
                    .and_then(|v| db::latest(v, None, false))
                    .cloned();
                to_api(p, latest.as_ref())
            })
            .collect(),
    ))
}

pub async fn create(
    State(state): State<AppState>,
    principal: Principal,
    Json(request): Json<CreatePackage>,
) -> Result<(StatusCode, Json<Package>)> {
    principal.require_admin()?;

    validate_package_name(&request.name).map_err(ApiError::BadRequest)?;
    validate_aliases(&request.aliases)?;
    let binary_name = request
        .binary_name
        .clone()
        .unwrap_or_else(|| request.name.clone());
    validate_binary_name(&binary_name)?;

    let row = sqlx::query_as!(
        PackageRow,
        "INSERT INTO cli_packages (name, description, binary_name, aliases, created_by)
         VALUES ($1, $2, $3, $4, $5)
         RETURNING id, name, description, binary_name, aliases, created_at, archived_at",
        request.name,
        request.description,
        binary_name,
        &request.aliases,
        principal.actor()
    )
    .fetch_one(&state.pool)
    .await
    .map_err(|e| match &e {
        sqlx::Error::Database(db) if db.is_unique_violation() => ApiError::Conflict(
            "already_exists",
            format!("package {} already exists", request.name),
        ),
        _ => e.into(),
    })?;

    audit::record_as(
        &state.pool,
        &principal,
        Event {
            action: "package.create",
            ecosystem: Some(ECOSYSTEM_CLI),
            package: Some(&request.name),
            version: None,
            detail: json!({ "binary_name": binary_name, "aliases": request.aliases }),
        },
    )
    .await?;

    Ok((StatusCode::CREATED, Json(to_api(row, None))))
}

pub async fn get(
    State(state): State<AppState>,
    principal: Principal,
    Path(name): Path<String>,
) -> Result<Json<PackageDetail>> {
    principal.require_reader()?;

    let package = db::find_package(&state.pool, &name).await?;
    let mut versions = db::versions(&state.pool, package.id).await?;
    versions.retain(VersionRow::is_published);
    versions.sort_by_key(|v| std::cmp::Reverse(v.semver()));

    let latest = db::latest(&versions, None, false).cloned();
    Ok(Json(PackageDetail {
        versions: versions.iter().map(VersionRow::summary).collect(),
        package: to_api(package, latest.as_ref()),
    }))
}

pub async fn update(
    State(state): State<AppState>,
    principal: Principal,
    Path(name): Path<String>,
    Json(request): Json<UpdatePackage>,
) -> Result<Json<Package>> {
    principal.require_admin()?;

    if let Some(aliases) = &request.aliases {
        validate_aliases(aliases)?;
    }
    if let Some(binary) = &request.binary_name {
        validate_binary_name(binary)?;
    }

    let row = sqlx::query_as!(
        PackageRow,
        "UPDATE cli_packages SET
            description = COALESCE($2, description),
            binary_name = COALESCE($3, binary_name),
            aliases = COALESCE($4, aliases),
            updated_at = now()
         WHERE name = $1
         RETURNING id, name, description, binary_name, aliases, created_at, archived_at",
        name,
        request.description,
        request.binary_name,
        request.aliases.as_deref()
    )
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| ApiError::NotFound(format!("package {name} not found")))?;

    audit::record_as(
        &state.pool,
        &principal,
        Event {
            action: "package.update",
            ecosystem: Some(ECOSYSTEM_CLI),
            package: Some(&name),
            version: None,
            detail: serde_json::to_value(&request).unwrap_or_default(),
        },
    )
    .await?;

    let versions = db::versions(&state.pool, row.id).await?;
    let latest = db::latest(&versions, None, false).cloned();
    Ok(Json(to_api(row, latest.as_ref())))
}

async fn set_archived(
    state: &AppState,
    principal: &Principal,
    name: &str,
    archived: bool,
) -> Result<Package> {
    principal.require_admin()?;

    let row = sqlx::query_as!(
        PackageRow,
        "UPDATE cli_packages SET
            archived_at = CASE WHEN $2 THEN COALESCE(archived_at, now()) END,
            updated_at = now()
         WHERE name = $1
         RETURNING id, name, description, binary_name, aliases, created_at, archived_at",
        name,
        archived
    )
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| ApiError::NotFound(format!("package {name} not found")))?;

    audit::record_as(
        &state.pool,
        principal,
        Event {
            action: if archived {
                "package.archive"
            } else {
                "package.unarchive"
            },
            ecosystem: Some(ECOSYSTEM_CLI),
            package: Some(name),
            version: None,
            detail: json!({}),
        },
    )
    .await?;

    let versions = db::versions(&state.pool, row.id).await?;
    let latest = db::latest(&versions, None, false).cloned();
    Ok(to_api(row, latest.as_ref()))
}

pub async fn archive(
    State(state): State<AppState>,
    principal: Principal,
    Path(name): Path<String>,
) -> Result<Json<Package>> {
    Ok(Json(set_archived(&state, &principal, &name, true).await?))
}

pub async fn unarchive(
    State(state): State<AppState>,
    principal: Principal,
    Path(name): Path<String>,
) -> Result<Json<Package>> {
    Ok(Json(set_archived(&state, &principal, &name, false).await?))
}

pub async fn delete(
    State(state): State<AppState>,
    principal: Principal,
    Path(name): Path<String>,
) -> Result<StatusCode> {
    principal.require_admin()?;

    let package = db::find_package(&state.pool, &name).await?;
    let versions = db::versions(&state.pool, package.id).await?;
    if versions.iter().any(VersionRow::is_published) {
        return Err(ApiError::Conflict(
            "has_published_versions",
            format!("{name} has published versions; archive it instead"),
        ));
    }

    for version in &versions {
        for artifact in db::artifacts(&state.pool, version.id).await? {
            state.storage.delete(&artifact.object_key).await?;
        }
    }

    sqlx::query!("DELETE FROM cli_packages WHERE id = $1", package.id)
        .execute(&state.pool)
        .await?;
    publishers::delete_for_package(&state.pool, ECOSYSTEM_CLI, &name).await?;

    audit::record_as(
        &state.pool,
        &principal,
        Event {
            action: "package.delete",
            ecosystem: Some(ECOSYSTEM_CLI),
            package: Some(&name),
            version: None,
            detail: json!({}),
        },
    )
    .await?;

    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub struct LatestQuery {
    #[serde(default)]
    target: Option<Target>,
    #[serde(default)]
    prerelease: bool,
}

pub async fn latest(
    State(state): State<AppState>,
    principal: Principal,
    Path(name): Path<String>,
    Query(query): Query<LatestQuery>,
) -> Result<Json<Version>> {
    principal.require_reader()?;

    let package = db::find_package(&state.pool, &name).await?;
    let versions = db::versions(&state.pool, package.id).await?;
    let version = db::latest(&versions, query.target, query.prerelease)
        .cloned()
        .ok_or_else(|| {
            ApiError::NotFound(match query.target {
                Some(target) => format!("{name} has no published version for {target}"),
                None => format!("{name} has no published version"),
            })
        })?;

    let artifacts = db::artifacts(&state.pool, version.id).await?;
    Ok(Json(version.into_api(&name, &artifacts)))
}
