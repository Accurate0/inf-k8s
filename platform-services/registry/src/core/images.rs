use axum::{
    Json,
    extract::{Query, State},
};
use chrono::{DateTime, Utc};
use registry_api::{Image, ImageRepository, Source};
use serde::Deserialize;
use sqlx::PgPool;
use uuid::Uuid;

use crate::{
    auth::Principal,
    error::{ApiError, Result},
    state::AppState,
};

struct Row {
    id: Uuid,
    repository: String,
    digest: String,
    source_repository: Option<String>,
    source_ref: Option<String>,
    source_sha: Option<String>,
    source_run_id: Option<String>,
    workflow_ref: Option<String>,
    signed_by: String,
    signed_by_kind: String,
    signed_at: DateTime<Utc>,
    attested_at: Option<DateTime<Utc>>,
}

impl From<Row> for Image {
    fn from(row: Row) -> Self {
        let source =
            (row.source_repository.is_some() || row.source_sha.is_some()).then_some(Source {
                repository: row.source_repository,
                git_ref: row.source_ref,
                sha: row.source_sha,
                run_id: row.source_run_id,
            });

        Image {
            id: row.id,
            repository: row.repository,
            digest: row.digest,
            source,
            workflow_ref: row.workflow_ref,
            signed_by: row.signed_by,
            signed_by_kind: row.signed_by_kind,
            signed_at: row.signed_at,
            attested_at: row.attested_at,
        }
    }
}

pub async fn record(
    pool: &PgPool,
    principal: &Principal,
    repository: &str,
    digest: &str,
) -> Result<()> {
    let claims = match principal {
        Principal::Forgejo(claims) => Some(claims),
        _ => None,
    };

    sqlx::query!(
        "INSERT INTO core_images (repository, digest, source_repository, source_ref, source_sha,
                                  source_run_id, workflow_ref, signed_by, signed_by_kind)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
         ON CONFLICT (repository, digest) DO UPDATE SET
             source_repository = EXCLUDED.source_repository,
             source_ref = EXCLUDED.source_ref,
             source_sha = EXCLUDED.source_sha,
             source_run_id = EXCLUDED.source_run_id,
             workflow_ref = EXCLUDED.workflow_ref
         WHERE core_images.source_repository IS NULL AND EXCLUDED.source_repository IS NOT NULL",
        repository,
        digest,
        claims.map(|c| c.repository.as_str()),
        claims.and_then(|c| c.git_ref.as_deref()),
        claims.and_then(|c| c.sha.as_deref()),
        claims.and_then(|c| c.run_id.as_deref()),
        claims.and_then(|c| c.workflow_ref.as_deref()),
        principal.actor(),
        principal.kind()
    )
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn mark_attested(pool: &PgPool, repository: &str, digest: &str) -> Result<()> {
    sqlx::query!(
        "UPDATE core_images SET attested_at = now()
         WHERE repository = $1 AND digest = $2 AND attested_at IS NULL",
        repository,
        digest
    )
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn repositories(pool: &PgPool, name: Option<&str>) -> Result<Vec<ImageRepository>> {
    let rows = sqlx::query!(
        r#"SELECT id AS "id!", repository AS "repository!", digest AS "digest!",
                  source_repository, source_ref, source_sha, source_run_id, workflow_ref,
                  signed_by AS "signed_by!", signed_by_kind AS "signed_by_kind!",
                  signed_at AS "signed_at!", attested_at, image_count AS "image_count!"
           FROM (
               SELECT DISTINCT ON (repository) *, count(*) OVER (PARTITION BY repository) AS image_count
               FROM core_images
               WHERE $1::text IS NULL OR repository = $1
               ORDER BY repository, signed_at DESC, id DESC
           ) latest
           ORDER BY signed_at DESC, repository"#,
        name
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|row| ImageRepository {
            name: row.repository.clone(),
            image_count: row.image_count,
            latest: Row {
                id: row.id,
                repository: row.repository,
                digest: row.digest,
                source_repository: row.source_repository,
                source_ref: row.source_ref,
                source_sha: row.source_sha,
                source_run_id: row.source_run_id,
                workflow_ref: row.workflow_ref,
                signed_by: row.signed_by,
                signed_by_kind: row.signed_by_kind,
                signed_at: row.signed_at,
                attested_at: row.attested_at,
            }
            .into(),
        })
        .collect())
}

pub async fn images(pool: &PgPool, repository: &str, limit: i64) -> Result<Vec<Image>> {
    let rows = sqlx::query_as!(
        Row,
        "SELECT id, repository, digest, source_repository, source_ref, source_sha, source_run_id,
                workflow_ref, signed_by, signed_by_kind, signed_at, attested_at
         FROM core_images
         WHERE repository = $1
         ORDER BY signed_at DESC, id DESC
         LIMIT $2",
        repository,
        limit
    )
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(Image::from).collect())
}

pub async fn find(pool: &PgPool, id: Uuid) -> Result<Image> {
    sqlx::query_as!(
        Row,
        "SELECT id, repository, digest, source_repository, source_ref, source_sha, source_run_id,
                workflow_ref, signed_by, signed_by_kind, signed_at, attested_at
         FROM core_images WHERE id = $1",
        id
    )
    .fetch_optional(pool)
    .await?
    .map(Image::from)
    .ok_or_else(|| ApiError::NotFound(format!("no image {id}")))
}

pub async fn list(
    State(state): State<AppState>,
    principal: Principal,
) -> Result<Json<Vec<ImageRepository>>> {
    principal.require_reader()?;

    Ok(Json(repositories(&state.pool, None).await?))
}

#[derive(Deserialize)]
pub struct RepositoryQuery {
    pub name: String,
    #[serde(default = "default_limit")]
    pub limit: i64,
}

fn default_limit() -> i64 {
    50
}

pub async fn repository(
    State(state): State<AppState>,
    principal: Principal,
    Query(query): Query<RepositoryQuery>,
) -> Result<Json<Vec<Image>>> {
    principal.require_reader()?;

    let images = images(&state.pool, &query.name, query.limit.clamp(1, 500)).await?;
    if images.is_empty() {
        return Err(ApiError::NotFound(format!(
            "no signed images in {}",
            query.name
        )));
    }

    Ok(Json(images))
}

#[derive(Deserialize)]
pub struct DigestQuery {
    pub repository: String,
    pub digest: String,
}

pub async fn digest(
    State(state): State<AppState>,
    principal: Principal,
    Query(query): Query<DigestQuery>,
) -> Result<Json<Image>> {
    principal.require_reader()?;

    sqlx::query_as!(
        Row,
        "SELECT id, repository, digest, source_repository, source_ref, source_sha, source_run_id,
                workflow_ref, signed_by, signed_by_kind, signed_at, attested_at
         FROM core_images WHERE repository = $1 AND digest = $2",
        query.repository,
        query.digest
    )
    .fetch_optional(&state.pool)
    .await?
    .map(|row| Json(row.into()))
    .ok_or_else(|| {
        ApiError::NotFound(format!(
            "{}@{} is not in the image ledger",
            query.repository, query.digest
        ))
    })
}
