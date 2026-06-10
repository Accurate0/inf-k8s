use std::collections::HashMap;
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
}

impl Registry {
    pub fn from_config(config: &Config) -> Self {
        let mut providers: HashMap<String, Arc<dyn Provider>> = HashMap::new();
        let mut routes: HashMap<(String, ModelKind), Vec<String>> = HashMap::new();

        for (name, pc) in &config.providers {
            let declared = pc
                .models
                .iter()
                .map(|m| (m, ModelKind::Chat))
                .chain(pc.embedding_models.iter().map(|m| (m, ModelKind::Embedding)));
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
        for names in routes.values_mut() {
            names.sort_by(|a, b| {
                let pa = config.providers.get(a).map(|p| p.priority).unwrap_or(i32::MAX);
                let pb = config.providers.get(b).map(|p| p.priority).unwrap_or(i32::MAX);
                pa.cmp(&pb).then_with(|| a.cmp(b))
            });
        }

        Self { providers, routes }
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Provider>> {
        self.providers.get(name).cloned()
    }

    /// Enabled providers that can serve `model` on this endpoint kind, in failover order.
    pub fn providers_for_model(&self, model: &str, kind: ModelKind) -> Vec<Arc<dyn Provider>> {
        self.routes
            .get(&(model.to_owned(), kind))
            .into_iter()
            .flatten()
            .filter_map(|name| self.get(name))
            .collect()
    }

    pub fn names(&self) -> Vec<String> {
        self.providers.keys().cloned().collect()
    }

    pub fn models(&self) -> Vec<(String, String)> {
        self.routes
            .iter()
            .flat_map(|((m, _), names)| names.iter().map(move |n| (m.clone(), n.clone())))
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
