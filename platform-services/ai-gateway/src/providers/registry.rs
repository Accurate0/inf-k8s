use std::collections::HashMap;
use std::sync::Arc;

use super::{Anthropic, Dialect, OpenAiCompatible, Provider};

/// The set of configured upstreams plus the default chosen per dialect. Providers are
/// enabled by the presence of their API key, keeping the secret surface declarative.
#[derive(Clone)]
pub struct Registry {
    providers: HashMap<String, Arc<dyn Provider>>,
    default_anthropic: Option<String>,
    default_openai: Option<String>,
}

impl Registry {
    pub fn from_env() -> Self {
        let mut builder = RegistryBuilder::default();

        if let Some(key) = env_key("ANTHROPIC_API_KEY") {
            let base = env_or("ANTHROPIC_BASE_URL", "https://api.anthropic.com");
            builder.add(Arc::new(Anthropic::new("anthropic", base, key)));
        }

        if let Some(key) = env_key("OPENAI_API_KEY") {
            let base = env_or("OPENAI_BASE_URL", "https://api.openai.com/v1");
            builder.add(Arc::new(OpenAiCompatible::new("openai", base, key)));
        }

        if let Some(key) = env_key("OPENROUTER_API_KEY") {
            let base = env_or("OPENROUTER_BASE_URL", "https://openrouter.ai/api/v1");
            builder.add(Arc::new(OpenAiCompatible::new("openrouter", base, key)));
        }

        if let Some(key) = env_key("GEMINI_API_KEY") {
            let base = env_or(
                "GEMINI_BASE_URL",
                "https://generativelanguage.googleapis.com/v1beta/openai",
            );
            builder.add(Arc::new(OpenAiCompatible::new("gemini", base, key)));
        }

        builder.build()
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Provider>> {
        self.providers.get(name).cloned()
    }

    pub fn default_for(&self, dialect: Dialect) -> Option<Arc<dyn Provider>> {
        let name = match dialect {
            Dialect::Anthropic => self.default_anthropic.as_deref(),
            Dialect::OpenAiCompatible => self.default_openai.as_deref(),
        }?;
        self.get(name)
    }

    pub fn names(&self) -> Vec<String> {
        self.providers.keys().cloned().collect()
    }
}

#[derive(Default)]
struct RegistryBuilder {
    providers: HashMap<String, Arc<dyn Provider>>,
    default_anthropic: Option<String>,
    default_openai: Option<String>,
}

impl RegistryBuilder {
    /// First provider registered for a dialect becomes that dialect's default.
    fn add(&mut self, provider: Arc<dyn Provider>) {
        let name = provider.name().to_owned();
        match provider.dialect() {
            Dialect::Anthropic => self.default_anthropic.get_or_insert(name.clone()),
            Dialect::OpenAiCompatible => self.default_openai.get_or_insert(name.clone()),
        };
        self.providers.insert(name, provider);
    }

    fn build(self) -> Registry {
        Registry {
            providers: self.providers,
            default_anthropic: self.default_anthropic,
            default_openai: self.default_openai,
        }
    }
}

fn env_key(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

fn env_or(name: &str, default: &str) -> String {
    match std::env::var(name) {
        Ok(v) if !v.is_empty() => v.trim_end_matches('/').to_owned(),
        _ => default.to_owned(),
    }
}
