use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use registry_api::{NewTrustedPublisher, TrustedPublisher};
use serde::Deserialize;
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use super::{
    audit::{self, Event},
    ensure_package_exists,
};
use crate::{
    auth::Principal,
    error::{ApiError, Result},
    state::AppState,
};

#[derive(Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    ecosystem: Option<String>,
    #[serde(default)]
    package: Option<String>,
}

pub async fn list(
    State(state): State<AppState>,
    principal: Principal,
    Query(query): Query<ListQuery>,
) -> Result<Json<Vec<TrustedPublisher>>> {
    principal.require_admin()?;

    let publishers = sqlx::query_as!(
        TrustedPublisher,
        "SELECT id, ecosystem, package, provider, issuer, repository, repository_id, ref_pattern,
                workflow, created_by, created_at
         FROM core_trusted_publishers
         WHERE ($1::text IS NULL OR ecosystem = $1) AND ($2::text IS NULL OR package = $2)
         ORDER BY ecosystem, package, created_at",
        query.ecosystem,
        query.package
    )
    .fetch_all(&state.pool)
    .await?;

    Ok(Json(publishers))
}

pub async fn create(
    State(state): State<AppState>,
    principal: Principal,
    Json(request): Json<NewTrustedPublisher>,
) -> Result<(StatusCode, Json<TrustedPublisher>)> {
    principal.require_admin()?;

    let issuer = state.auth.forgejo_issuer().ok_or_else(|| {
        ApiError::BadRequest("forgejo trusted publishing is not configured".to_owned())
    })?;

    if request
        .repository
        .split('/')
        .filter(|s| !s.is_empty())
        .count()
        != 2
    {
        return Err(ApiError::BadRequest(format!(
            "repository must be owner/repo, got {:?}",
            request.repository
        )));
    }

    ensure_package_exists(&state.pool, &request.ecosystem, &request.package).await?;

    let publisher = sqlx::query_as!(
        TrustedPublisher,
        "INSERT INTO core_trusted_publishers (ecosystem, package, issuer, repository, ref_pattern, workflow, created_by)
         VALUES ($1, $2, $3, $4, $5, $6, $7)
         RETURNING id, ecosystem, package, provider, issuer, repository, repository_id, ref_pattern,
                   workflow, created_by, created_at",
        request.ecosystem,
        request.package,
        issuer,
        request.repository,
        request.ref_pattern,
        request.workflow,
        principal.actor()
    )
    .fetch_one(&state.pool)
    .await
    .map_err(|e| match &e {
        sqlx::Error::Database(db) if db.is_unique_violation() => ApiError::Conflict(
            "already_exists",
            "an identical trusted publisher already exists".to_owned(),
        ),
        _ => e.into(),
    })?;

    audit::record_as(
        &state.pool,
        &principal,
        Event {
            action: "trusted_publisher.create",
            ecosystem: Some(&request.ecosystem),
            package: Some(&request.package),
            version: None,
            detail: json!({
                "id": publisher.id,
                "repository": request.repository,
                "ref_pattern": request.ref_pattern,
                "workflow": request.workflow,
            }),
        },
    )
    .await?;

    Ok((StatusCode::CREATED, Json(publisher)))
}

pub async fn delete(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<Uuid>,
) -> Result<StatusCode> {
    principal.require_admin()?;

    let row = sqlx::query!(
        "DELETE FROM core_trusted_publishers WHERE id = $1 RETURNING ecosystem, package",
        id
    )
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| ApiError::NotFound(format!("no trusted publisher {id}")))?;

    audit::record_as(
        &state.pool,
        &principal,
        Event {
            action: "trusted_publisher.delete",
            ecosystem: Some(&row.ecosystem),
            package: Some(&row.package),
            version: None,
            detail: json!({ "id": id }),
        },
    )
    .await?;

    Ok(StatusCode::NO_CONTENT)
}

pub async fn delete_for_package(pool: &PgPool, ecosystem: &str, package: &str) -> Result<()> {
    sqlx::query!(
        "DELETE FROM core_trusted_publishers WHERE ecosystem = $1 AND package = $2",
        ecosystem,
        package
    )
    .execute(pool)
    .await?;
    sqlx::query!(
        "DELETE FROM core_publish_token_scopes WHERE ecosystem = $1 AND package = $2",
        ecosystem,
        package
    )
    .execute(pool)
    .await?;
    Ok(())
}
