use std::collections::HashMap;

use axum::{
    Json,
    extract::{Query, State},
};
use chrono::{DateTime, Utc};
use registry_api::{
    HarborAction, PromoteRequest, Promoted, Promotion,
    oci::{DOCKER_MANIFEST_LIST, Fetched, INDEX, Index, Manifest},
};
use serde::Deserialize;
use serde_json::json;
use sqlx::PgPool;

use super::audit::{self, Event};
use super::exchange::{configured, image_project};
use crate::{
    auth::{Principal, grants::allowed, oidc::groups_match},
    cli::cosign,
    config::DeployConfig,
    error::{ApiError, Result},
    harbor::Harbor,
    state::AppState,
};

const COMMIT_SHA: &str = "net.inf-k8s.commit-sha";
const COMMITTED_AT: &str = "net.inf-k8s.committed-at";
const MAX_CONFIG_BYTES: u64 = 1024 * 1024;
const MAX_LIMIT: i64 = 200;

fn deploy_config(state: &AppState) -> Result<&DeployConfig> {
    state
        .config
        .deploy
        .as_ref()
        .ok_or_else(|| ApiError::BadRequest("promotion is not configured".to_owned()))
}

fn valid_app(app: &str) -> bool {
    !app.is_empty()
        && app.len() <= 63
        && app.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
        && app
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn authorize(state: &AppState, principal: &Principal, project: &str) -> Result<()> {
    let (_, harbor) = configured(state)?;
    let deploy = deploy_config(state)?;

    if !allowed(&harbor.grants, principal, project).contains(&HarborAction::Push) {
        return Err(ApiError::Forbidden(format!(
            "{} has no push grant on harbor project {project}",
            principal.actor()
        )));
    }

    let grouped = matches!(
        principal,
        Principal::User { .. } | Principal::Exchanged { .. }
    );
    if grouped && !groups_match(principal.groups(), &deploy.groups) {
        return Err(ApiError::Forbidden(format!(
            "{} is not in a deploy group ({})",
            principal.actor(),
            deploy.groups.join(", ")
        )));
    }
    Ok(())
}

#[derive(Default)]
struct Labels {
    sha: Option<String>,
    committed_at: Option<DateTime<Utc>>,
}

async fn labels(harbor: &Harbor, repository: &str, fetched: &Fetched) -> Result<Labels> {
    let body = if [INDEX, DOCKER_MANIFEST_LIST].contains(&fetched.media_type.as_str()) {
        let index: Index = serde_json::from_slice(&fetched.body)
            .map_err(|e| ApiError::Harbor(format!("{repository}: unreadable index: {e}")))?;
        let image = index
            .manifests
            .iter()
            .find(|m| m.platform.as_ref().is_none_or(|p| p.os != "unknown"));
        let Some(image) = image else {
            return Ok(Labels::default());
        };
        match harbor.oci().manifest(repository, &image.digest).await? {
            Some(manifest) => manifest.body,
            None => return Ok(Labels::default()),
        }
    } else {
        fetched.body.clone()
    };

    let Ok(manifest) = serde_json::from_slice::<Manifest>(&body) else {
        return Ok(Labels::default());
    };

    #[derive(Deserialize)]
    struct ImageConfig {
        #[serde(default)]
        config: Option<Inner>,
    }

    #[derive(Deserialize)]
    struct Inner {
        #[serde(default, rename = "Labels")]
        labels: Option<HashMap<String, String>>,
    }

    let config = harbor
        .oci()
        .blob_bytes(repository, &manifest.config.digest, MAX_CONFIG_BYTES)
        .await?;
    let labels = serde_json::from_slice::<ImageConfig>(&config)
        .ok()
        .and_then(|c| c.config)
        .and_then(|c| c.labels)
        .unwrap_or_default();

    Ok(Labels {
        sha: labels.get(COMMIT_SHA).cloned(),
        committed_at: labels
            .get(COMMITTED_AT)
            .and_then(|at| at.parse::<i64>().ok())
            .and_then(|at| DateTime::from_timestamp(at, 0)),
    })
}

pub async fn promote(
    state: &AppState,
    principal: &Principal,
    request: PromoteRequest,
) -> Result<Promoted> {
    let (harbor, _) = configured(state)?;
    let deploy = deploy_config(state)?;
    if !valid_app(&request.app) {
        return Err(ApiError::BadRequest(format!(
            "{:?} is not an app name",
            request.app
        )));
    }
    let repository = &request.repository;
    let project = image_project(repository).map_err(ApiError::BadRequest)?;
    authorize(state, principal, project)?;

    let fetched = harbor
        .oci()
        .manifest(repository, &request.reference)
        .await?
        .ok_or_else(|| {
            ApiError::NotFound(format!(
                "{repository}:{} is not in harbor",
                request.reference
            ))
        })?;
    let digest = fetched.digest.clone();

    let signature = cosign::signature_tag(&digest);
    if harbor
        .oci()
        .manifest(repository, &signature)
        .await?
        .is_none()
    {
        return Err(ApiError::Unprocessable(format!(
            "{repository}@{digest} is not signed; sign it before promoting"
        )));
    }

    let mut tx = state.pool.begin().await?;
    sqlx::query!("SELECT pg_advisory_xact_lock(hashtext($1))", repository)
        .execute(&mut *tx)
        .await?;

    let current = sqlx::query_as!(
        Promotion,
        r#"SELECT id, app, repository, digest, sha, committed_at, promoted_at,
                  NULL::timestamptz AS "live_until?", actor, forced
           FROM core_promotions WHERE repository = $1
           ORDER BY promoted_at DESC, id DESC LIMIT 1"#,
        repository
    )
    .fetch_optional(&mut *tx)
    .await?;

    if let Some(current) = current.clone()
        && current.digest == digest
    {
        return Ok(Promoted {
            previous: Some(current.digest.clone()),
            promotion: current,
            changed: false,
        });
    }

    let labels = labels(harbor, repository, &fetched).await?;
    if !request.force
        && let Some(current) = &current
        && let (Some(candidate), Some(live)) = (labels.committed_at, current.committed_at)
        && candidate < live
    {
        return Err(ApiError::Conflict(
            "older_build",
            format!(
                "{repository}@{digest} was committed before the deployed {}; pass force to roll back",
                current.sha.as_deref().unwrap_or(&current.digest)
            ),
        ));
    }

    harbor
        .oci()
        .put_manifest(repository, &deploy.tag, &fetched.media_type, &fetched.body)
        .await?;

    let promotion = sqlx::query_as!(
        Promotion,
        r#"INSERT INTO core_promotions
               (app, repository, digest, sha, committed_at, actor, actor_kind, forced)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
           RETURNING id, app, repository, digest, sha, committed_at, promoted_at,
                     NULL::timestamptz AS "live_until?", actor, forced"#,
        request.app,
        repository,
        digest,
        labels.sha,
        labels.committed_at,
        principal.actor(),
        principal.kind(),
        request.force
    )
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;

    let previous = current.map(|c| c.digest);
    audit::record_as(
        &state.pool,
        principal,
        Event {
            action: "harbor.promote",
            ecosystem: None,
            package: None,
            version: None,
            detail: json!({
                "app": promotion.app,
                "repository": repository,
                "digest": digest,
                "sha": promotion.sha,
                "previous": previous,
                "forced": request.force,
            }),
        },
    )
    .await?;
    tracing::info!(
        repository,
        digest,
        actor = principal.actor(),
        forced = request.force,
        "promoted image"
    );

    if let Err(e) = dispatch_pending(state).await {
        tracing::warn!("publish dispatch failed, retrying in the background: {e}");
    }

    Ok(Promoted {
        promotion,
        previous,
        changed: true,
    })
}

pub async fn promotions(
    pool: &PgPool,
    repositories: &[String],
    limit: i64,
) -> Result<Vec<Promotion>> {
    let promotions = sqlx::query_as!(
        Promotion,
        r#"SELECT id AS "id!", app AS "app!", repository AS "repository!", digest AS "digest!",
                  sha, committed_at, promoted_at AS "promoted_at!", live_until AS "live_until?",
                  actor AS "actor!", forced AS "forced!"
           FROM (
               SELECT *,
                      LEAD(promoted_at) OVER (PARTITION BY repository ORDER BY promoted_at, id)
                          AS live_until,
                      ROW_NUMBER() OVER (PARTITION BY repository ORDER BY promoted_at DESC, id DESC)
                          AS n
               FROM core_promotions WHERE repository = ANY($1)
           ) p
           WHERE n <= $2
           ORDER BY repository, promoted_at DESC, id DESC"#,
        repositories,
        limit.clamp(1, MAX_LIMIT)
    )
    .fetch_all(pool)
    .await?;
    Ok(promotions)
}

pub async fn dispatch_pending(state: &AppState) -> Result<usize> {
    let Some(dispatch) = state
        .config
        .deploy
        .as_ref()
        .and_then(|d| d.dispatch.as_ref())
    else {
        return Ok(0);
    };
    let Some(token) = &state.dispatch_token else {
        return Ok(0);
    };

    let pending = sqlx::query_scalar!("SELECT id FROM core_promotions WHERE dispatched_at IS NULL")
        .fetch_all(&state.pool)
        .await?;
    if pending.is_empty() {
        return Ok(0);
    }

    let url = format!(
        "{}/api/v1/repos/{}/actions/workflows/{}/dispatches",
        dispatch.url.trim_end_matches('/'),
        dispatch.repository,
        dispatch.workflow
    );
    let response = state
        .http
        .post(&url)
        .header("Authorization", format!("token {token}"))
        .json(&json!({ "ref": dispatch.git_ref }))
        .send()
        .await
        .map_err(|e| ApiError::Harbor(format!("dispatching {}: {e}", dispatch.workflow)))?;
    if !response.status().is_success() {
        return Err(ApiError::Harbor(format!(
            "dispatching {} failed ({}): {}",
            dispatch.workflow,
            response.status(),
            response.text().await.unwrap_or_default()
        )));
    }

    sqlx::query!(
        "UPDATE core_promotions SET dispatched_at = now() WHERE id = ANY($1)",
        &pending
    )
    .execute(&state.pool)
    .await?;
    tracing::info!(
        promotions = pending.len(),
        "dispatched {}",
        dispatch.workflow
    );
    Ok(pending.len())
}

pub async fn create(
    State(state): State<AppState>,
    principal: Principal,
    Json(request): Json<PromoteRequest>,
) -> Result<Json<Promoted>> {
    Ok(Json(promote(&state, &principal, request).await?))
}

#[derive(Deserialize)]
pub struct ListQuery {
    pub repository: String,
    #[serde(default = "default_limit")]
    pub limit: i64,
}

fn default_limit() -> i64 {
    20
}

pub async fn list(
    State(state): State<AppState>,
    principal: Principal,
    Query(query): Query<ListQuery>,
) -> Result<Json<Vec<Promotion>>> {
    principal.require_reader()?;
    let repositories: Vec<String> = query
        .repository
        .split(',')
        .filter(|r| !r.is_empty())
        .map(str::to_owned)
        .collect();
    Ok(Json(
        promotions(&state.pool, &repositories, query.limit).await?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_names_are_dns_labels() {
        assert!(valid_app("spotify"));
        assert!(valid_app("ai-gateway"));
        assert!(!valid_app(""));
        assert!(!valid_app("-x"));
        assert!(!valid_app("Spotify"));
        assert!(!valid_app("a/b"));
        assert!(!valid_app(&"a".repeat(64)));
    }
}
