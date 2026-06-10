use std::time::Duration;

use sqlx::PgPool;

use crate::{
    cache::CacheClient, config::Config, feature_flag::FeatureFlagClient, keys::KeyStore,
    providers::Registry,
};

/// Shared, cheaply-cloneable application state handed to every handler.
#[derive(Clone)]
pub struct AppState {
    pub config: Config,
    pub providers: Registry,
    pub pool: PgPool,
    pub keys: KeyStore,
    pub features: FeatureFlagClient,
    pub cache: Option<CacheClient>,
    pub http: reqwest::Client,
}

impl AppState {
    pub fn new(
        config: Config,
        providers: Registry,
        pool: PgPool,
        features: FeatureFlagClient,
        cache: Option<CacheClient>,
    ) -> Self {
        let http = reqwest::Client::builder()
            // LLM responses stream for a long time; don't impose a total deadline,
            // only a connect timeout.
            .connect_timeout(Duration::from_secs(10))
            .build()
            .expect("failed to build http client");

        Self {
            keys: KeyStore::new(pool.clone()),
            config,
            providers,
            pool,
            features,
            cache,
            http,
        }
    }
}
