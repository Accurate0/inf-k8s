use open_feature::{EvaluationContext, OpenFeature, provider::NoOpProvider};
use openfeature_provider::{EvaluationMode, FeatureFlagProvider};
use std::sync::Arc;

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
        fields(otel.name = format!("flag {flag}"), result = tracing::field::Empty)
    )]
    pub async fn bool_flag(&self, flag: &str, key_name: &str, default: bool) -> bool {
        let span = tracing::Span::current();
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
        result
    }

    #[tracing::instrument(
        skip(self),
        fields(otel.name = format!("flag {flag}"), result = tracing::field::Empty)
    )]
    pub async fn string_flag(&self, flag: &str, key_name: &str, default: &str) -> String {
        let span = tracing::Span::current();

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
        result
    }
}
