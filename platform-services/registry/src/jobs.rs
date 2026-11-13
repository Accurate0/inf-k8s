use std::time::Duration;

use crate::{cli::catalog, error::Result, state::AppState};

async fn retry_catalog(state: AppState) {
    let mut ticker = tokio::time::interval(CATALOG_INTERVAL);
    loop {
        ticker.tick().await;
        if let Err(e) = catalog::render_if_dirty(&state).await {
            tracing::warn!("catalog render failed: {e}");
        }
    }
}

const INTERVAL: Duration = Duration::from_secs(3600);

const CATALOG_INTERVAL: Duration = Duration::from_secs(60);

pub async fn run(state: AppState) {
    tokio::spawn(retry_catalog(state.clone()));

    let mut ticker = tokio::time::interval(INTERVAL);
    loop {
        ticker.tick().await;
        match collect_stale_drafts(&state).await {
            Ok(0) => {}
            Ok(removed) => tracing::info!(removed, "garbage collected stale drafts"),
            Err(e) => tracing::warn!("draft garbage collection failed: {e}"),
        }
        if state.harbor.is_some() {
            match catalog::backfill(&state).await {
                Ok(0) => {}
                Ok(done) => tracing::info!(done, "backfilled versions into harbor"),
                Err(e) => tracing::warn!("harbor backfill failed: {e}"),
            }
            match catalog::sign_unsigned(&state).await {
                Ok(0) => {}
                Ok(signed) => tracing::info!(signed, "cosign-signed releases in harbor"),
                Err(e) => tracing::warn!("cosign signing failed: {e}"),
            }
        }
        match collect_expired_robots(&state).await {
            Ok(0) => {}
            Ok(removed) => tracing::info!(removed, "deleted expired harbor robots"),
            Err(e) => tracing::warn!("harbor robot garbage collection failed: {e}"),
        }
    }
}

pub async fn collect_stale_drafts(state: &AppState) -> Result<usize> {
    let drafts = sqlx::query_scalar!(
        "SELECT id FROM cli_versions
         WHERE status = 'draft' AND created_at < now() - make_interval(hours => $1::int)",
        state.config.limits.draft_ttl_hours as i32
    )
    .fetch_all(&state.pool)
    .await?;

    for id in &drafts {
        let keys = sqlx::query_scalar!(
            "SELECT object_key FROM cli_artifacts WHERE version_id = $1",
            id
        )
        .fetch_all(&state.pool)
        .await?;
        for key in keys {
            if crate::cli::release::oci_manifest(&key).is_none() {
                state.storage.delete(&key).await?;
            }
        }
        sqlx::query!(
            "DELETE FROM cli_versions WHERE id = $1 AND status = 'draft'",
            id
        )
        .execute(&state.pool)
        .await?;
    }

    Ok(drafts.len())
}

pub async fn collect_expired_robots(state: &AppState) -> Result<usize> {
    let Some(harbor) = &state.harbor else {
        return Ok(0);
    };

    let expired = sqlx::query_scalar!(
        "SELECT id FROM core_harbor_robots WHERE deleted_at IS NULL AND expires_at < now()"
    )
    .fetch_all(&state.pool)
    .await?;

    for id in &expired {
        harbor.delete_robot(*id).await?;
        sqlx::query!(
            "UPDATE core_harbor_robots SET deleted_at = now() WHERE id = $1",
            id
        )
        .execute(&state.pool)
        .await?;
    }

    Ok(expired.len())
}
