use std::collections::HashMap;

use serde::Deserialize;

use crate::providers::Dialect;

/// Providers and their model routing are compiled in from `config.yaml`; the admin token
/// comes from the environment.
const CONFIG_YAML: &str = include_str!("../config.yaml");

#[derive(Clone, Debug, Default)]
pub struct Config {
    pub admin_token: String,
    pub providers: HashMap<String, ProviderConfig>,
    pub keys: Vec<KeyConfig>,
}

#[derive(Debug, Default, Deserialize)]
struct FileConfig {
    #[serde(default)]
    providers: HashMap<String, ProviderConfig>,
    #[serde(default)]
    keys: Vec<KeyConfig>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ProviderConfig {
    pub dialect: Dialect,
    pub base_url: String,
    #[serde(default)]
    pub api_key_env: Option<String>,
    #[serde(default)]
    pub models: Vec<String>,
    #[serde(default)]
    pub embedding_models: Vec<String>,
}

impl ProviderConfig {
    /// Env var holding this provider's API key, defaulting to `<NAME>_API_KEY`.
    pub fn api_key_env(&self, name: &str) -> String {
        self.api_key_env
            .clone()
            .unwrap_or_else(|| format!("{}_API_KEY", name.to_uppercase()))
    }
}

/// Claims an existing key (minted via the admin API) by name and manages its allowed
/// models and budget; config is the source of truth for those fields.
#[derive(Clone, Debug, Deserialize)]
pub struct KeyConfig {
    pub name: String,
    #[serde(default)]
    pub allowed_models: Vec<String>,
    #[serde(default)]
    pub monthly_token_budget: Option<i64>,
    #[serde(default)]
    pub revoked: bool,
}

impl Config {
    pub fn load() -> anyhow::Result<Self> {
        let file: FileConfig = serde_yaml::from_str(CONFIG_YAML)
            .map_err(|e| anyhow::anyhow!("failed to parse config.yaml: {e}"))?;

        Ok(Self {
            admin_token: std::env::var("ADMIN_TOKEN").unwrap_or_default(),
            providers: file.providers,
            keys: file.keys,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_config_parses() {
        let config = Config::load().unwrap();
        assert!(!config.providers.is_empty());
    }
}
