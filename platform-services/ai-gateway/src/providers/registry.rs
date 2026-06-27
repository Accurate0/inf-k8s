use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use super::{Anthropic, Dialect, ModelKind, OpenAiCompatible, Provider};
use crate::config::Config;

/// Configured upstreams and the routing table. Routes are keyed by `(model, kind)` so an
/// embedding model is unreachable from chat endpoints, and map to providers in failover
/// order.
#[derive(Clone, Default)]
pub struct Registry {
    providers: HashMap<String, Arc<dyn Provider>>,
    routes: HashMap<(String, ModelKind), Vec<String>>,
    /// Providers serving any otherwise-unrouted model, in failover order.
    fallbacks: Vec<String>,
}

impl Registry {
    pub fn from_config(config: &Config) -> Self {
        let mut providers: HashMap<String, Arc<dyn Provider>> = HashMap::new();
        let mut routes: HashMap<(String, ModelKind), Vec<String>> = HashMap::new();
        let mut fallbacks: Vec<String> = Vec::new();

        for (name, pc) in &config.providers {
            if pc.fallback {
                fallbacks.push(name.clone());
            }
            let declared = pc.models.iter().map(|m| (m, ModelKind::Chat)).chain(
                pc.embedding_models
                    .iter()
                    .map(|m| (m, ModelKind::Embedding)),
            );
            for (model, kind) in declared {
                routes
                    .entry((model.clone(), kind))
                    .or_default()
                    .push(name.clone());
            }

            let key_env = pc.api_key_env(name);
            let Some(key) = env_value(&key_env) else {
                tracing::warn!(
                    provider = name,
                    env = key_env,
                    "skipping provider: API key env var unset"
                );
                continue;
            };

            let base = pc.base_url.trim_end_matches('/').to_owned();
            let provider: Arc<dyn Provider> = match pc.dialect {
                Dialect::Anthropic => Arc::new(Anthropic::new(name.clone(), base, key)),
                Dialect::OpenAiCompatible => {
                    Arc::new(OpenAiCompatible::new(name.clone(), base, key))
                }
            };
            providers.insert(name.clone(), provider);
        }

        // Lowest priority first, name as a deterministic tiebreaker.
        let by_priority = |a: &String, b: &String| {
            let pa = config
                .providers
                .get(a)
                .map(|p| p.priority)
                .unwrap_or(i32::MAX);
            let pb = config
                .providers
                .get(b)
                .map(|p| p.priority)
                .unwrap_or(i32::MAX);
            pa.cmp(&pb).then_with(|| a.cmp(b))
        };
        for names in routes.values_mut() {
            names.sort_by(&by_priority);
        }
        fallbacks.sort_by(&by_priority);

        Self {
            providers,
            routes,
            fallbacks,
        }
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Provider>> {
        self.providers.get(name).cloned()
    }

    /// Enabled providers that can serve `model` on this endpoint kind, in failover order.
    /// When no provider explicitly declares the model, the fallback providers are returned.
    pub fn providers_for_model(&self, model: &str, kind: ModelKind) -> Vec<Arc<dyn Provider>> {
        let declared: Vec<_> = self
            .routes
            .get(&(model.to_owned(), kind))
            .into_iter()
            .flatten()
            .filter_map(|name| self.get(name))
            .collect();
        if !declared.is_empty() {
            return declared;
        }
        self.fallbacks
            .iter()
            .filter_map(|name| self.get(name))
            .collect()
    }

    pub fn names(&self) -> Vec<String> {
        self.providers.keys().cloned().collect()
    }

    /// Unique routable models, each paired with the provider serving its highest-priority
    /// route, sorted by model id. A model reachable through several providers or endpoint
    /// kinds appears once.
    pub fn models(&self) -> Vec<(String, String)> {
        let mut by_model: BTreeMap<&str, &str> = BTreeMap::new();
        for ((model, _), names) in &self.routes {
            if let Some(primary) = names.first() {
                by_model.entry(model).or_insert(primary);
            }
        }
        by_model
            .into_iter()
            .map(|(m, p)| (m.to_owned(), p.to_owned()))
            .collect()
    }
}

fn env_value(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry_from_yaml(yaml: &str, key_env: &str) -> Registry {
        let providers: HashMap<String, crate::config::ProviderConfig> =
            serde_yaml::from_str(yaml).unwrap();
        let config = Config {
            providers,
            ..Default::default()
        };
        unsafe { std::env::set_var(key_env, "secret") };
        Registry::from_config(&config)
    }

    #[test]
    fn routes_model_to_its_provider() {
        let yaml = r#"
anthropic:
  dialect: anthropic
  base_url: https://example.test
  api_key_env: TEST_ANTHROPIC_KEY
  models:
    - claude-fable-5
"#;
        let registry = registry_from_yaml(yaml, "TEST_ANTHROPIC_KEY");
        let providers = registry.providers_for_model("claude-fable-5", ModelKind::Chat);
        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0].name(), "anthropic");
        assert!(
            registry
                .providers_for_model("unknown-model", ModelKind::Chat)
                .is_empty()
        );
    }

    #[test]
    fn embedding_models_route_only_from_embedding_endpoint() {
        let yaml = r#"
openai:
  dialect: openai
  base_url: https://example.test
  api_key_env: TEST_OPENAI_KEY
  embedding_models:
    - text-embedding-3-large
"#;
        let registry = registry_from_yaml(yaml, "TEST_OPENAI_KEY");
        assert!(
            !registry
                .providers_for_model("text-embedding-3-large", ModelKind::Embedding)
                .is_empty()
        );
        assert!(
            registry
                .providers_for_model("text-embedding-3-large", ModelKind::Chat)
                .is_empty()
        );
    }

    #[test]
    fn unrouted_models_fall_back_to_fallback_providers() {
        let yaml = r#"
openai:
  dialect: openai
  base_url: https://openai.test
  api_key_env: TEST_FALLBACK_KEY
  models:
    - gpt-4o
openrouter:
  dialect: openai
  base_url: https://openrouter.test
  api_key_env: TEST_FALLBACK_KEY
  fallback: true
"#;
        let registry = registry_from_yaml(yaml, "TEST_FALLBACK_KEY");

        // Declared models still route to their own provider.
        let providers = registry.providers_for_model("gpt-4o", ModelKind::Chat);
        let names: Vec<_> = providers.iter().map(|p| p.name()).collect();
        assert_eq!(names, ["openai"]);

        // An unknown model falls back to the fallback provider.
        let providers = registry.providers_for_model("some-new-model", ModelKind::Chat);
        let names: Vec<_> = providers.iter().map(|p| p.name()).collect();
        assert_eq!(names, ["openrouter"]);
    }

    #[test]
    fn providers_for_model_are_ordered_by_priority() {
        let yaml = r#"
openrouter:
  dialect: openai
  base_url: https://openrouter.test
  api_key_env: TEST_FAILOVER_KEY
  priority: 200
  models:
    - gpt-4o
openai:
  dialect: openai
  base_url: https://openai.test
  api_key_env: TEST_FAILOVER_KEY
  priority: 10
  models:
    - gpt-4o
"#;
        let registry = registry_from_yaml(yaml, "TEST_FAILOVER_KEY");
        let providers = registry.providers_for_model("gpt-4o", ModelKind::Chat);
        let names: Vec<_> = providers.iter().map(|p| p.name()).collect();
        assert_eq!(names, ["openai", "openrouter"]);
    }
}
