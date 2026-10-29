use axum::{
    Json,
    extract::{Query, State},
};
use registry_api::AuditEntry;
use serde::Deserialize;
use serde_json::Value;
use sqlx::PgPool;

use crate::{auth::Principal, error::Result, state::AppState};

pub struct Event<'a> {
    pub action: &'a str,
    pub ecosystem: Option<&'a str>,
    pub package: Option<&'a str>,
    pub version: Option<&'a str>,
    pub detail: Value,
}

pub async fn record(pool: &PgPool, actor: &str, actor_kind: &str, event: Event<'_>) -> Result<()> {
    sqlx::query!(
        "INSERT INTO core_audit_log (actor, actor_kind, action, ecosystem, package, version, detail)
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
        actor,
        actor_kind,
        event.action,
        event.ecosystem,
        event.package,
        event.version,
        event.detail
    )
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn record_as(pool: &PgPool, principal: &Principal, event: Event<'_>) -> Result<()> {
    record(pool, &principal.actor(), principal.kind(), event).await
}

#[derive(Deserialize)]
pub struct AuditQuery {
    #[serde(default)]
    ecosystem: Option<String>,
    #[serde(default)]
    package: Option<String>,
    #[serde(default = "default_limit")]
    limit: i64,
}

fn default_limit() -> i64 {
    100
}

pub async fn list(
    State(state): State<AppState>,
    principal: Principal,
    Query(query): Query<AuditQuery>,
) -> Result<Json<Vec<AuditEntry>>> {
    principal.require_admin()?;

    let entries = sqlx::query_as!(
        AuditEntry,
        "SELECT id, at, actor, actor_kind, action, ecosystem, package, version, detail
         FROM core_audit_log
         WHERE ($1::text IS NULL OR ecosystem = $1) AND ($2::text IS NULL OR package = $2)
         ORDER BY at DESC, id DESC
         LIMIT $3",
        query.ecosystem,
        query.package,
        query.limit.clamp(1, 1000)
    )
    .fetch_all(&state.pool)
    .await?;

    Ok(Json(entries))
}
