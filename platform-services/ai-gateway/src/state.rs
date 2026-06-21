use std::time::Duration;

use sqlx::PgPool;

use crate::{
    cache::CacheClient, config::Config, feature_flag::FeatureFlagClient, keys::KeyStore,
    pricing::Pricing, providers::Registry,
};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const IDLE_TIMEOUT: Duration = Duration::from_secs(120);

/// Shared, cheaply-cloneable application state handed to every handler.
#[derive(Clone)]
pub struct AppState {
    pub config: Config,
    pub providers: Registry,
    pub pool: PgPool,
    pub keys: KeyStore,
    pub features: FeatureFlagClient,
    pub pricing: Pricing,
    /// Shared response/cache store, also handed to [`KeyStore`]. `None` when `REDIS_URL`
    /// is unset, in which case response caching is off and lookups hit Postgres.
    pub cache: Option<CacheClient>,
    pub http: reqwest::Client,
}

impl AppState {
    pub fn new(
        config: Config,
        providers: Registry,
        pool: PgPool,
        features: FeatureFlagClient,
        pricing: Pricing,
        cache: Option<CacheClient>,
    ) -> Self {
        // No total deadline (streams run long), but a stalled connection — no bytes for
        // IDLE_TIMEOUT — fails so it can't pin a task and client connection forever.
        let http = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .read_timeout(IDLE_TIMEOUT)
            .build()
            .expect("failed to build http client");

        Self {
            keys: KeyStore::new(pool.clone(), cache.clone()),
            config,
            providers,
            pool,
            features,
            pricing,
            cache,
            http,
        }
    }
}
