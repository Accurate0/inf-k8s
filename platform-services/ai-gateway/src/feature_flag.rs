use std::{sync::Arc, time::Duration};

use moka::future::Cache;
use open_feature::{EvaluationContext, OpenFeature, provider::NoOpProvider};
use openfeature_provider::{EvaluationMode, FeatureFlagProvider};

/// Runtime routing / kill-switch flags evaluated per request, backed by the
/// feature-flags gRPC service via its OpenFeature provider. Falls back to a NoOp
/// provider (every flag returns its default) when `FEATURE_FLAGS_URL` is unset.
///
/// The provider runs in [`Local`](EvaluationMode::Local) mode: it streams the flag
/// snapshot from the backend and evaluates in-process, so the per-request hot path
/// never makes a network round-trip.
#[derive(Clone)]
pub struct FeatureFlagClient {
    client: Arc<open_feature::Client>,
    environment: &'static str,
    bool_cache: Cache<String, bool>,
    string_cache: Cache<String, String>,
}

impl FeatureFlagClient {
    pub async fn from_env() -> Self {
        Self::new(std::env::var("FEATURE_FLAGS_URL").ok()).await
    }

    pub async fn new(url: Option<String>) -> Self {
        let mut client = OpenFeature::singleton_mut().await;

        if let Some(url) = url {
            match FeatureFlagProvider::connect_with(url, EvaluationMode::Local).await {
                Ok(provider) => client.set_provider(provider).await,
                Err(e) => {
                    tracing::error!("error when connecting to feature-flags: {e}");
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

    #[tracing::instrument(
        skip(self),
        fields(otel.name = format!("flag {flag}"), result = tracing::field::Empty, cached = tracing::field::Empty)
    )]
    pub async fn bool_flag(&self, flag: &str, key_name: &str, default: bool) -> bool {
        let span = tracing::Span::current();
        let cache_key = format!("{flag}:{key_name}");
        if let Some(v) = self.bool_cache.get(&cache_key).await {
            span.record("result", v);
            span.record("cached", true);
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

        span.record("result", result);
        span.record("cached", false);
        self.bool_cache.insert(cache_key, result).await;
        result
    }

    #[tracing::instrument(
        skip(self),
        fields(otel.name = format!("flag {flag}"), result = tracing::field::Empty, cached = tracing::field::Empty)
    )]
    pub async fn string_flag(&self, flag: &str, key_name: &str, default: &str) -> String {
        let span = tracing::Span::current();
        let cache_key = format!("{flag}:{key_name}");
        if let Some(v) = self.string_cache.get(&cache_key).await {
            span.record("result", v.as_str());
            span.record("cached", true);
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

        span.record("result", result.as_str());
        span.record("cached", false);
        self.string_cache.insert(cache_key, result.clone()).await;
        result
    }
}
