use std::{sync::Arc, time::Duration};

use moka::future::Cache;
use open_feature::{EvaluationContext, EvaluationContextFieldValue, OpenFeature, provider::NoOpProvider};
use open_feature_flipt::flipt::{self, FliptProvider, NoneAuthentication};

/// Runtime routing / kill-switch flags evaluated per request. Falls back to a NoOp
/// provider (every flag returns its default) when `FLIPT_URL` is unset, mirroring
/// janitor-bot.
#[derive(Clone)]
pub struct FeatureFlagClient {
    client: Arc<open_feature::Client>,
    environment: &'static str,
    bool_cache: Cache<String, bool>,
    string_cache: Cache<String, String>,
}

impl FeatureFlagClient {
    pub async fn from_env() -> Self {
        Self::new(std::env::var("FLIPT_URL").ok()).await
    }

    pub async fn new(url: Option<String>) -> Self {
        let mut client = OpenFeature::singleton_mut().await;

        if let Some(url) = url {
            let config = flipt::Config {
                url,
                authentication_strategy: NoneAuthentication::new(),
                timeout: 60,
            };

            match FliptProvider::new("ai-gateway".to_string(), config) {
                Ok(provider) => client.set_provider(provider).await,
                Err(e) => {
                    tracing::error!("error when init flipt: {e}");
                    client.set_provider(NoOpProvider::default()).await
                }
            };
        } else {
            tracing::warn!("fallback to noop feature provider");
            client.set_provider(NoOpProvider::default()).await;
        }

        let environment = if cfg!(debug_assertions) {
            "development"
        } else {
            "production"
        };

        Self {
            client: Arc::new(client.create_client()),
            environment,
            bool_cache: Cache::builder()
                .name("ff_bool_cache")
                .time_to_live(Duration::from_secs(30))
                .build(),
            string_cache: Cache::builder()
                .name("ff_string_cache")
                .time_to_live(Duration::from_secs(30))
                .build(),
        }
    }

    fn context(&self, key_name: &str) -> EvaluationContext {
        EvaluationContext::default()
            .with_targeting_key(key_name)
            .with_custom_field("environment", self.environment)
            .with_custom_field("key", key_name)
    }

    /// Global kill switch et al. Cached briefly; cache key folds in the virtual key
    /// name so per-key targeting still works.
    pub async fn bool_flag(&self, flag: &str, key_name: &str, default: bool) -> bool {
        let cache_key = format!("{flag}:{key_name}");
        if let Some(v) = self.bool_cache.get(&cache_key).await {
            return v;
        }

        let result = match self
            .client
            .get_bool_value(flag, Some(&self.context(key_name)), None)
            .await
        {
            Ok(v) => v,
            Err(e) => {
                tracing::debug!("flag {flag} eval error, using default {default}: {e:?}");
                default
            }
        };

        self.bool_cache.insert(cache_key, result).await;
        result
    }

    /// Variant flag returning a string (e.g. a model or provider override). An empty
    /// string is treated as "no override" by callers.
    pub async fn string_flag(&self, flag: &str, key_name: &str, default: &str) -> String {
        let cache_key = format!("{flag}:{key_name}");
        if let Some(v) = self.string_cache.get(&cache_key).await {
            return v;
        }

        let result = match self
            .client
            .get_string_value(flag, Some(&self.context(key_name)), None)
            .await
        {
            Ok(v) => v,
            Err(e) => {
                tracing::debug!("flag {flag} eval error, using default: {e:?}");
                default.to_owned()
            }
        };

        self.string_cache.insert(cache_key, result.clone()).await;
        result
    }
}

// Silence unused import warning when the open-feature API shifts; kept for clarity.
#[allow(unused)]
type FieldValue = EvaluationContextFieldValue;
