use std::time::Duration;

use crate::{error::Result, state::AppState};

const INTERVAL: Duration = Duration::from_secs(3600);

pub async fn run(state: AppState) {
    let mut ticker = tokio::time::interval(INTERVAL);
    loop {
        ticker.tick().await;
        match collect_stale_drafts(&state).await {
            Ok(0) => {}
            Ok(removed) => tracing::info!(removed, "garbage collected stale drafts"),
            Err(e) => tracing::warn!("draft garbage collection failed: {e}"),
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
            state.storage.delete(&key).await?;
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
