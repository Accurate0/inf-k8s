use std::{fmt, str::FromStr};

use chrono::{DateTime, TimeDelta, Utc};
use serde::{Deserialize, Serialize};
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
    /// Provider-native request body sent upstream; None on cache hits.
    pub request_body: Option<String>,
    /// Provider-native response body received from upstream; None on cache hits.
    pub response_body: Option<String>,
}

/// Inserts a usage row. Logged-and-swallowed on failure: telemetry must never break
/// the proxy path.
#[tracing::instrument(skip_all, fields(otel.name = "usage.record"))]
pub async fn record(pool: &PgPool, event: &UsageEvent) {
    let result = sqlx::query!(
        r#"INSERT INTO usage_events
         (key_id, key_name, provider, requested_model, resolved_model,
          input_tokens, output_tokens, latency_ms, status, cost_usd, cache_hit,
          request_body, response_body)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)"#,
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
        event.request_body.as_deref(),
        event.response_body.as_deref(),
    )
    .execute(pool)
    .await;

    if let Err(e) = result {
        tracing::error!("failed to record usage event: {e}");
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    amount: i64,
    unit: char,
}

impl Window {
    const MAX_SECONDS: i64 = 3650 * 86_400;

    fn unit_seconds(unit: char) -> Option<i64> {
        match unit {
            'm' => Some(60),
            'h' => Some(3600),
            'd' => Some(86_400),
            'w' => Some(604_800),
            _ => None,
        }
    }

    pub fn seconds(&self) -> i64 {
        self.amount * Self::unit_seconds(self.unit).unwrap_or(0)
    }

    pub fn start(&self) -> DateTime<Utc> {
        Utc::now() - TimeDelta::seconds(self.seconds())
    }
}

impl Default for Window {
    fn default() -> Self {
        Self {
            amount: 7,
            unit: 'd',
        }
    }
}

impl fmt::Display for Window {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", self.amount, self.unit)
    }
}

impl FromStr for Window {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        let invalid =
            || format!("invalid window `{s}`; expected a number and unit like 30m, 24h, 7d or 2w");

        let Some(unit) = s.chars().last() else {
            return Err(invalid());
        };

        let Some(unit_seconds) = Self::unit_seconds(unit) else {
            return Err(invalid());
        };

        let digits = &s[..s.len() - unit.len_utf8()];

        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return Err(invalid());
        }

        let amount: i64 = digits.parse().map_err(|_| invalid())?;

        if amount == 0 {
            return Err(invalid());
        }

        match amount.checked_mul(unit_seconds) {
            Some(seconds) if seconds <= Self::MAX_SECONDS => Ok(Self { amount, unit }),
            _ => Err(format!("window `{s}` is too large; the maximum is 3650d")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageRow {
    pub key_name: String,
    pub model: String,
    pub requests: i64,
    pub cache_hits: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cost_usd: f64,
}

pub async fn summary(pool: &PgPool, window: Window) -> Result<Vec<UsageRow>> {
    let rows = sqlx::query_as!(
        UsageRow,
        r#"SELECT key_name,
                  resolved_model AS model,
                  COUNT(*)::bigint AS "requests!",
                  COUNT(*) FILTER (WHERE cache_hit)::bigint AS "cache_hits!",
                  COALESCE(SUM(input_tokens), 0)::bigint AS "input_tokens!",
                  COALESCE(SUM(output_tokens), 0)::bigint AS "output_tokens!",
                  COALESCE(SUM(cost_usd), 0)::double precision AS "cost_usd!"
           FROM usage_events
           WHERE created_at >= $1
           GROUP BY key_name, resolved_model
           ORDER BY key_name, 3 DESC, resolved_model"#,
        window.start(),
    )
    .fetch_all(pool)
    .await?;

    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_parses_each_unit() {
        let seconds = |s: &str| s.parse::<Window>().unwrap().seconds();

        assert_eq!(seconds("30m"), 1800);
        assert_eq!(seconds("24h"), 86_400);
        assert_eq!(seconds("7d"), 604_800);
        assert_eq!(seconds("2w"), 1_209_600);
    }

    #[test]
    fn window_round_trips_through_display() {
        for text in ["30m", "24h", "7d", "2w"] {
            assert_eq!(text.parse::<Window>().unwrap().to_string(), text);
        }

        assert_eq!(Window::default().to_string(), "7d");
    }

    #[test]
    fn window_rejects_malformed_input() {
        for text in [
            "", "7", "d", "0d", "-1d", "+1d", "1.5h", "7 d", "7y", "7dd", "7é",
        ] {
            assert!(text.parse::<Window>().is_err(), "{text:?} should not parse");
        }
    }

    #[test]
    fn window_rejects_oversized_spans() {
        assert!("3650d".parse::<Window>().is_ok());
        assert!("3651d".parse::<Window>().is_err());
        assert!("9223372036854775807w".parse::<Window>().is_err());
    }
}
