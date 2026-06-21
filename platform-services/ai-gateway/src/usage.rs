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
    pub latency_ms: i64,
    pub status: i32,
    /// Estimated USD cost from the price table; 0 for cache hits and unpriced models.
    pub cost_usd: f64,
    /// True when served from the response cache without an upstream call.
    pub cache_hit: bool,
}

/// Inserts a usage row. Logged-and-swallowed on failure: telemetry must never break
/// the proxy path.
#[tracing::instrument(skip_all, fields(otel.name = "usage.record"))]
pub async fn record(pool: &PgPool, event: &UsageEvent) {
    let result = sqlx::query!(
        r#"INSERT INTO usage_events
         (key_id, key_name, provider, requested_model, resolved_model,
          input_tokens, output_tokens, latency_ms, status, cost_usd, cache_hit)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)"#,
        event.key_id,
        &event.key_name,
        &event.provider,
        &event.requested_model,
        &event.resolved_model,
        event.input_tokens,
        event.output_tokens,
        event.latency_ms,
        event.status,
        event.cost_usd,
        event.cache_hit,
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
    pub cache_hits: Option<i64>,
    pub cost_usd: Option<f64>,
}

/// Live per-day usage for `/admin/usage`, bounded to the last 30 days to keep the
/// hypertable scan cheap.
pub async fn summary(pool: &PgPool) -> Result<Vec<UsageRow>> {
    let rows = sqlx::query_as!(
        UsageRow,
        "SELECT key_name, \
                resolved_model AS model, \
                date_trunc('day', created_at)::date AS day, \
                SUM(input_tokens)::bigint AS input_tokens, \
                SUM(output_tokens)::bigint AS output_tokens, \
                COUNT(*)::bigint AS requests, \
                COUNT(*) FILTER (WHERE cache_hit)::bigint AS cache_hits, \
                SUM(cost_usd)::double precision AS cost_usd \
         FROM usage_events \
         WHERE created_at >= now() - interval '30 days' \
         GROUP BY key_name, resolved_model, date_trunc('day', created_at)::date \
         ORDER BY day DESC, key_name, model LIMIT 1000",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}
