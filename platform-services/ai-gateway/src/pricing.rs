use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sqlx::PgPool;

use crate::error::Result;
use crate::providers::Usage;

/// How often the in-memory price map is refreshed from the database. Prices change at most
/// daily (the upstream sync), so a coarse refresh keeps every replica current without putting
/// the lookup on the request hot path.
const REFRESH_INTERVAL: Duration = Duration::from_secs(15 * 60);

const SYNC_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

pub const UPSTREAM: &str = "https://www.llm-prices.com/current-v1.json";

/// One entry from llm-prices.com `current-v1.json`. Rates are USD per million tokens.
#[derive(Deserialize)]
struct UpstreamPrice {
    id: String,
    input: Option<f64>,
    output: Option<f64>,
    input_cached: Option<f64>,
}

#[derive(Deserialize)]
struct UpstreamPrices {
    prices: Vec<UpstreamPrice>,
}

/// A model's rates in USD per one million tokens, as stored in `model_prices` and supplied
/// by the pricing sync. `cached` is the discounted rate for cache-read input tokens, when
/// the upstream publishes one.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ModelPrice {
    pub id: String,
    pub input_usd_per_mtok: f64,
    pub output_usd_per_mtok: f64,
    #[serde(default)]
    pub cached_usd_per_mtok: Option<f64>,
}

/// Resolved-model -> price lookup, shared across handlers and refreshed in the background.
/// Empty until the first load succeeds; an unknown model costs zero rather than erroring,
/// so a missing price never breaks the proxy path.
#[derive(Clone, Default)]
pub struct Pricing {
    prices: Arc<RwLock<HashMap<String, ModelPrice>>>,
}

impl Pricing {
    /// Loads the current price table from the database into a fresh map.
    pub async fn load(pool: &PgPool) -> Self {
        let pricing = Self::default();
        if let Err(e) = pricing.refresh(pool).await {
            tracing::error!("failed to load model prices, starting empty: {e}");
        }
        pricing
    }

    /// Replaces the in-memory map with the current contents of `model_prices`.
    pub async fn refresh(&self, pool: &PgPool) -> Result<()> {
        let rows = sqlx::query_as!(
            ModelPrice,
            "SELECT id, input_usd_per_mtok, output_usd_per_mtok, cached_usd_per_mtok \
             FROM model_prices",
        )
        .fetch_all(pool)
        .await?;

        let map = rows.into_iter().map(|p| (p.id.clone(), p)).collect();
        *self.prices.write().unwrap() = map;
        Ok(())
    }

    /// Refreshes from the database every [`REFRESH_INTERVAL`] so replicas pick up a sync
    /// without a restart.
    pub fn spawn_refresh(&self, pool: PgPool) {
        let pricing = self.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(REFRESH_INTERVAL);
            ticker.tick().await; // the immediate first tick; initial load already ran
            loop {
                ticker.tick().await;
                if let Err(e) = pricing.refresh(&pool).await {
                    tracing::warn!("periodic price refresh failed: {e}");
                }
            }
        });
    }

    /// Estimated USD cost of a request given its token usage. Returns 0 when the resolved
    /// model has no known price.
    pub fn cost(&self, model: &str, usage: Usage) -> f64 {
        let prices = self.prices.read().unwrap();
        let Some(price) = prices.get(model) else {
            return 0.0;
        };
        let input = usage.input.max(0) as f64 / 1_000_000.0 * price.input_usd_per_mtok;
        let output = usage.output.max(0) as f64 / 1_000_000.0 * price.output_usd_per_mtok;
        input + output
    }
}

/// Fetches current prices from [`UPSTREAM`] and upserts every model priced for both input and
/// output. Returns the number of rows written.
pub async fn sync(http: &reqwest::Client, pool: &PgPool, pricing: &Pricing) -> Result<u64> {
    let upstream: UpstreamPrices = http
        .get(UPSTREAM)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    let prices: Vec<ModelPrice> = upstream
        .prices
        .into_iter()
        .filter_map(|p| {
            Some(ModelPrice {
                id: p.id,
                input_usd_per_mtok: p.input?,
                output_usd_per_mtok: p.output?,
                cached_usd_per_mtok: p.input_cached,
            })
        })
        .collect();

    let written = upsert(pool, &prices).await?;
    pricing.refresh(pool).await?;
    Ok(written)
}

/// Syncs from upstream once a day; every replica does, and the upsert is idempotent.
pub fn spawn_sync(http: reqwest::Client, pool: PgPool, pricing: Pricing) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(SYNC_INTERVAL);
        loop {
            ticker.tick().await;
            match sync(&http, &pool, &pricing).await {
                Ok(written) => tracing::info!(written, "synced model prices from {UPSTREAM}"),
                Err(e) => tracing::warn!("model price sync failed: {e}"),
            }
        }
    });
}

/// Upserts a batch of prices (from the sync) and refreshes the in-memory map. Returns the
/// number of rows written.
pub async fn upsert(pool: &PgPool, prices: &[ModelPrice]) -> Result<u64> {
    let mut written = 0;
    for price in prices {
        sqlx::query!(
            "INSERT INTO model_prices \
                (id, input_usd_per_mtok, output_usd_per_mtok, cached_usd_per_mtok, updated_at) \
             VALUES ($1, $2, $3, $4, now()) \
             ON CONFLICT (id) DO UPDATE SET \
                input_usd_per_mtok = EXCLUDED.input_usd_per_mtok, \
                output_usd_per_mtok = EXCLUDED.output_usd_per_mtok, \
                cached_usd_per_mtok = EXCLUDED.cached_usd_per_mtok, \
                updated_at = now()",
            price.id,
            price.input_usd_per_mtok,
            price.output_usd_per_mtok,
            price.cached_usd_per_mtok,
        )
        .execute(pool)
        .await?;
        written += 1;
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pricing_with(model: &str, input: f64, output: f64) -> Pricing {
        let pricing = Pricing::default();
        pricing.prices.write().unwrap().insert(
            model.to_owned(),
            ModelPrice {
                id: model.to_owned(),
                input_usd_per_mtok: input,
                output_usd_per_mtok: output,
                cached_usd_per_mtok: None,
            },
        );
        pricing
    }

    #[test]
    fn cost_uses_per_million_rates() {
        let pricing = pricing_with("claude-opus-4-8", 15.0, 75.0);
        let usage = Usage {
            input: 1_000_000,
            output: 2_000_000,
        };
        // 1M input @ $15 + 2M output @ $75 = 15 + 150.
        assert!((pricing.cost("claude-opus-4-8", usage) - 165.0).abs() < 1e-9);
    }

    #[test]
    fn unknown_model_is_free() {
        let pricing = pricing_with("known", 1.0, 1.0);
        assert_eq!(
            pricing.cost(
                "unknown",
                Usage {
                    input: 5,
                    output: 5
                }
            ),
            0.0
        );
    }
}
