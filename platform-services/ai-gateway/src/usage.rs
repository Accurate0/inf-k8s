use chrono::NaiveDate;
use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::Result;

/// One billable interaction, written after the upstream response completes.
#[derive(Debug, Clone)]
pub struct UsageEvent {
    pub key_id: Option<Uuid>,
    pub key_name: String,
    pub provider: String,
    pub requested_model: String,
    pub resolved_model: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_hit: bool,
    pub latency_ms: i64,
    pub status: i32,
}

/// Inserts a usage row. Logged-and-swallowed on failure: telemetry must never break
/// the proxy path.
#[tracing::instrument(skip_all, fields(otel.name = "usage.record"))]
pub async fn record(pool: &PgPool, event: &UsageEvent) {
    let result = sqlx::query!(
        r#"INSERT INTO usage_events
         (key_id, key_name, provider, requested_model, resolved_model,
          input_tokens, output_tokens, cache_hit, latency_ms, status)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)"#,
        event.key_id,
        &event.key_name,
        &event.provider,
        &event.requested_model,
        &event.resolved_model,
        event.input_tokens,
        event.output_tokens,
        event.cache_hit,
        event.latency_ms,
        event.status,
    )
    .execute(pool)
    .await;

    if let Err(e) = result {
        tracing::error!("failed to record usage event: {e}");
    }
}

#[derive(Serialize, sqlx::FromRow)]
pub struct UsageRow {
    pub key_name: String,
    pub model: String,
    pub day: Option<NaiveDate>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub requests: Option<i64>,
}

/// Live per-day usage for `/admin/usage`, aggregated straight from `usage_events` so
/// results reflect traffic immediately rather than waiting on the rollup. Cheap on the
/// hypertable; the rolled-up `usage_daily` table is reserved for Grafana.
pub async fn summary(pool: &PgPool) -> Result<Vec<UsageRow>> {
    let rows = sqlx::query_as!(
        UsageRow,
        "SELECT key_name, \
                resolved_model AS model, \
                date_trunc('day', created_at)::date AS day, \
                SUM(input_tokens)::bigint AS input_tokens, \
                SUM(output_tokens)::bigint AS output_tokens, \
                COUNT(*)::bigint AS requests \
         FROM usage_events \
         GROUP BY key_name, resolved_model, date_trunc('day', created_at)::date \
         ORDER BY day DESC, key_name, model LIMIT 1000",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Refreshes `usage_daily` from `usage_events`. Run periodically so Grafana queries
/// stay cheap regardless of `usage_events` cardinality.
pub async fn refresh_rollup(pool: &PgPool) -> Result<()> {
    sqlx::query!(
        "INSERT INTO usage_daily (key_name, model, day, input_tokens, output_tokens, requests) \
         SELECT key_name, resolved_model, date_trunc('day', created_at)::date, \
                SUM(input_tokens), SUM(output_tokens), COUNT(*) \
         FROM usage_events \
         GROUP BY key_name, resolved_model, date_trunc('day', created_at)::date \
         ON CONFLICT (key_name, model, day) DO UPDATE SET \
            input_tokens = EXCLUDED.input_tokens, \
            output_tokens = EXCLUDED.output_tokens, \
            requests = EXCLUDED.requests",
    )
    .execute(pool)
    .await?;
    Ok(())
}
