use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use chrono::Utc;
use registry_api::{CreateToken, CreatedToken, Scope, TokenInfo};
use serde_json::json;
use uuid::Uuid;

use super::{
    audit::{self, Event},
    ensure_package_exists,
};
use crate::{
    auth::{Principal, tokens},
    error::{ApiError, Result},
    state::AppState,
};

pub async fn create(
    State(state): State<AppState>,
    principal: Principal,
    Json(request): Json<CreateToken>,
) -> Result<(StatusCode, Json<CreatedToken>)> {
    principal.require_admin()?;

    if request.name.trim().is_empty() {
        return Err(ApiError::BadRequest("token name is required".to_owned()));
    }
    if request.scopes.is_empty() {
        return Err(ApiError::BadRequest(
            "a token needs at least one package scope".to_owned(),
        ));
    }
    for scope in &request.scopes {
        ensure_package_exists(&state.pool, &scope.ecosystem, &scope.package).await?;
    }

    let token = tokens::generate();
    let expires_at = request
        .expires_in_days
        .map(|days| Utc::now() + chrono::Duration::days(days));

    let mut tx = state.pool.begin().await?;
    let id = sqlx::query_scalar!(
        "INSERT INTO core_publish_tokens (name, token_hash, token_prefix, created_by, expires_at)
         VALUES ($1, $2, $3, $4, $5) RETURNING id",
        request.name,
        tokens::hash(&token),
        tokens::display_prefix(&token),
        principal.actor(),
        expires_at
    )
    .fetch_one(&mut *tx)
    .await?;

    for scope in &request.scopes {
        sqlx::query!(
            "INSERT INTO core_publish_token_scopes (token_id, ecosystem, package) VALUES ($1, $2, $3)
             ON CONFLICT DO NOTHING",
            id,
            scope.ecosystem,
            scope.package
        )
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;

    audit::record_as(
        &state.pool,
        &principal,
        Event {
            action: "token.create",
            ecosystem: None,
            package: None,
            version: None,
            detail: json!({ "id": id, "name": request.name, "scopes": request.scopes }),
        },
    )
    .await?;

    Ok((StatusCode::CREATED, Json(CreatedToken { id, token })))
}

pub async fn list(
    State(state): State<AppState>,
    principal: Principal,
) -> Result<Json<Vec<TokenInfo>>> {
    principal.require_admin()?;

    let rows = sqlx::query!(
        "SELECT id, name, token_prefix, created_by, created_at, expires_at, last_used_at, revoked_at
         FROM core_publish_tokens ORDER BY created_at DESC"
    )
    .fetch_all(&state.pool)
    .await?;

    let scopes = sqlx::query!("SELECT token_id, ecosystem, package FROM core_publish_token_scopes")
        .fetch_all(&state.pool)
        .await?;

    Ok(Json(
        rows.into_iter()
            .map(|row| TokenInfo {
                scopes: scopes
                    .iter()
                    .filter(|s| s.token_id == row.id)
                    .map(|s| Scope {
                        ecosystem: s.ecosystem.clone(),
                        package: s.package.clone(),
                    })
                    .collect(),
                id: row.id,
                name: row.name,
                prefix: row.token_prefix,
                created_by: row.created_by,
                created_at: row.created_at,
                expires_at: row.expires_at,
                last_used_at: row.last_used_at,
                revoked_at: row.revoked_at,
            })
            .collect(),
    ))
}

pub async fn revoke(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<Uuid>,
) -> Result<StatusCode> {
    principal.require_admin()?;

    let revoked = sqlx::query!(
        "UPDATE core_publish_tokens SET revoked_at = now() WHERE id = $1 AND revoked_at IS NULL",
        id
    )
    .execute(&state.pool)
    .await?
    .rows_affected();

    if revoked == 0 {
        return Err(ApiError::NotFound(format!("no active token {id}")));
    }

    audit::record_as(
        &state.pool,
        &principal,
        Event {
            action: "token.revoke",
            ecosystem: None,
            package: None,
            version: None,
            detail: json!({ "id": id }),
        },
    )
    .await?;

    Ok(StatusCode::NO_CONTENT)
}
