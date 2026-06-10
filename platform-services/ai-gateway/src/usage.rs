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
pub async fn record(pool: &PgPool, event: &UsageEvent) {
    let result = sqlx::query(
        "INSERT INTO usage_events \
         (key_id, key_name, provider, requested_model, resolved_model, \
          input_tokens, output_tokens, cache_hit, latency_ms, status) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
    )
    .bind(event.key_id)
    .bind(&event.key_name)
    .bind(&event.provider)
    .bind(&event.requested_model)
    .bind(&event.resolved_model)
    .bind(event.input_tokens)
    .bind(event.output_tokens)
    .bind(event.cache_hit)
    .bind(event.latency_ms)
    .bind(event.status)
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
    pub day: NaiveDate,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub requests: i64,
}

/// Reads the rolled-up daily usage table for `/admin/usage`.
pub async fn summary(pool: &PgPool) -> Result<Vec<UsageRow>> {
    let rows = sqlx::query_as::<_, UsageRow>(
        "SELECT key_name, model, day, input_tokens, output_tokens, requests \
         FROM usage_daily ORDER BY day DESC, key_name, model LIMIT 1000",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Refreshes `usage_daily` from `usage_events`. Run periodically so Grafana queries
/// stay cheap regardless of `usage_events` cardinality.
pub async fn refresh_rollup(pool: &PgPool) -> Result<()> {
    sqlx::query(
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
