use std::collections::HashMap;
use std::sync::Arc;

use super::{Anthropic, Dialect, OpenAiCompatible, Provider};
use crate::config::Config;

/// Configured upstreams plus the model→provider routing table. A provider is enabled
/// once its API key is present in the environment.
#[derive(Clone, Default)]
pub struct Registry {
    providers: HashMap<String, Arc<dyn Provider>>,
    routes: HashMap<String, String>,
}

impl Registry {
    pub fn from_config(config: &Config) -> Self {
        let mut providers: HashMap<String, Arc<dyn Provider>> = HashMap::new();
        let mut routes: HashMap<String, String> = HashMap::new();

        for (name, pc) in &config.providers {
            for model in &pc.models {
                if let Some(existing) = routes.insert(model.clone(), name.clone()) {
                    tracing::warn!(
                        model,
                        existing,
                        replacement = name,
                        "model declared under multiple providers; last one wins"
                    );
                }
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

        Self { providers, routes }
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Provider>> {
        self.providers.get(name).cloned()
    }

    pub fn provider_for_model(&self, model: &str) -> Option<Arc<dyn Provider>> {
        let provider = self.routes.get(model)?;
        self.get(provider)
    }

    pub fn names(&self) -> Vec<String> {
        self.providers.keys().cloned().collect()
    }

    pub fn models(&self) -> Vec<(String, String)> {
        self.routes
            .iter()
            .map(|(m, p)| (m.clone(), p.clone()))
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
        let provider = registry.provider_for_model("claude-fable-5").unwrap();
        assert_eq!(provider.name(), "anthropic");
        assert!(registry.provider_for_model("unknown-model").is_none());
    }
}
