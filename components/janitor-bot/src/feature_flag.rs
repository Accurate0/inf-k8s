use std::{sync::Arc, time::Duration};

use moka::sync::Cache;
use open_feature::{EvaluationContext, OpenFeature, provider::NoOpProvider};
use open_feature_flipt::flipt::{self, FliptProvider, NoneAuthentication};

#[derive(Clone)]
pub struct FeatureFlagClient {
    client: Arc<open_feature::Client>,
    evaluation_context: EvaluationContext,
    cache: Cache<String, bool>,
}

impl FeatureFlagClient {
    pub async fn from_env() -> Self {
        let url = std::env::var("FLIPT_URL");

        Self::new(url.ok()).await
    }

    pub async fn new(url: Option<String>) -> Self {
        let mut client = OpenFeature::singleton_mut().await;

        if let Some(url) = url {
            let config = flipt::Config {
                url,
                authentication_strategy: NoneAuthentication::new(),
                timeout: 60,
            };

            match FliptProvider::new("janitor-bot".to_string(), config) {
                Ok(provider) => client.set_provider(provider).await,
                Err(e) => {
                    tracing::error!("error when init flipt: {e}");
                    client.set_provider(NoOpProvider::default()).await
                }
            };
        } else {
            tracing::warn!("fallback to noop feature provider");
            client.set_provider(NoOpProvider::default()).await;
        };

        let client = client.create_client();

        let evaluation_context = EvaluationContext::default().with_custom_field(
            "environment",
            if cfg!(debug_assertions) {
                "development"
            } else {
                "production"
            },
        );

        Self {
            client: Arc::new(client),
            evaluation_context,
            cache: Cache::builder()
                .name("feature_flag_cache")
                .time_to_live(Duration::from_secs(60))
                .build(),
        }
    }

    pub async fn is_feature_enabled(
        &self,
        feature_flag: &str,
        default: bool,
        mut evaluation_context: EvaluationContext,
    ) -> bool {
        let cached_evaluation = self.cache.get(feature_flag);

        if let Some(cached_evaluation) = cached_evaluation {
            tracing::info!("evaluated {feature_flag} as {cached_evaluation} from cache");
            cached_evaluation
        } else {
            evaluation_context.merge_missing(&self.evaluation_context);

            let server_evaluation = self
                .client
                .get_bool_value(feature_flag, Some(&evaluation_context), None)
                .await;

            let evaluation_result = match server_evaluation {
                Ok(eval) => eval,
                Err(e) => {
                    tracing::error!("error evaluating: {feature_flag} because {e:?}");
                    default
                }
            };

            self.cache
                .insert(feature_flag.to_string(), evaluation_result);
            tracing::info!("evaluated {feature_flag} as {evaluation_result}");

            evaluation_result
        }
    }
}
