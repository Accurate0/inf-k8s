use std::collections::HashMap;

use serde::Deserialize;

use crate::providers::Dialect;

/// Providers and their model routing are baked in from `config.yaml`; the admin token
/// comes from the environment. The baked-in copy is the always-available fallback when
/// no ConfigMap is mounted (see [`Config::load`]).
const CONFIG_YAML: &str = include_str!("../config.yaml");

/// When set, the config file at this path (a mounted ConfigMap) is preferred over the
/// baked-in copy, provided its `version` matches [`CONFIG_SCHEMA_VERSION`].
const CONFIG_PATH_ENV: &str = "CONFIG_PATH";

/// Config-format version this binary understands. Bump on any breaking change to the
/// config schema. A ConfigMap whose `version` differs is rejected in favour of the
/// baked-in config. See [`Config::load`].
pub const CONFIG_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Default)]
pub struct Config {
    pub admin_token: String,
    pub providers: HashMap<String, ProviderConfig>,
    pub keys: Vec<KeyConfig>,
    /// Global requested-model -> resolved-model remapping, applied to every key. A
    /// per-key entry in [`KeyConfig::model_overrides`] takes precedence.
    pub model_overrides: HashMap<String, String>,
}

#[derive(Debug, Default, Deserialize)]
struct FileConfig {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    providers: HashMap<String, ProviderConfig>,
    #[serde(default)]
    keys: Vec<KeyConfig>,
    #[serde(default)]
    model_overrides: HashMap<String, String>,
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
    /// Failover order among providers that serve the same model: lower is tried first.
    #[serde(default = "default_priority")]
    pub priority: i32,
}

fn default_priority() -> i32 {
    100
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
    /// Per-key requested-model -> resolved-model remapping; takes precedence over
    /// [`Config::model_overrides`].
    #[serde(default)]
    pub model_overrides: HashMap<String, String>,
}

impl Config {
    /// Load the active config. When `CONFIG_PATH` is set, the ConfigMap at that path
    /// wins, provided its `version` matches [`CONFIG_SCHEMA_VERSION`]; a version
    /// mismatch (breaking schema change) falls back to the baked-in config, while a
    /// missing or malformed ConfigMap errors so the pod fails to start and the
    /// previous ReplicaSet keeps running. When unset, uses the baked-in config.
    pub fn load() -> anyhow::Result<Self> {
        let file = Self::load_file()?;

        Ok(Self {
            admin_token: std::env::var("ADMIN_TOKEN").unwrap_or_default(),
            providers: file.providers,
            keys: file.keys,
            model_overrides: file.model_overrides,
        })
    }

    /// Config-driven override of the requested model: the per-key map wins, falling
    /// back to the global map. Returns `None` when no override applies.
    pub fn override_model(&self, key_name: &str, requested: &str) -> Option<&str> {
        self.keys
            .iter()
            .find(|k| k.name == key_name)
            .and_then(|k| k.model_overrides.get(requested))
            .or_else(|| self.model_overrides.get(requested))
            .map(String::as_str)
    }

    fn load_file() -> anyhow::Result<FileConfig> {
        let Ok(path) = std::env::var(CONFIG_PATH_ENV) else {
            tracing::info!("{CONFIG_PATH_ENV} unset; using baked-in config");
            return Self::baked_in_config();
        };

        let contents = std::fs::read_to_string(&path)
            .map_err(|e| anyhow::anyhow!("failed to read config ConfigMap at {path}: {e}"))?;
        let file: FileConfig = serde_yaml::from_str(&contents)
            .map_err(|e| anyhow::anyhow!("failed to parse config ConfigMap at {path}: {e}"))?;

        if file.version != CONFIG_SCHEMA_VERSION {
            tracing::warn!(
                configmap_version = file.version,
                code_version = CONFIG_SCHEMA_VERSION,
                "config ConfigMap version incompatible with this binary (breaking change); using baked-in config"
            );
            return Self::baked_in_config();
        }

        tracing::info!(path, version = file.version, "loaded config from ConfigMap");
        Ok(file)
    }

    fn baked_in_config() -> anyhow::Result<FileConfig> {
        serde_yaml::from_str(CONFIG_YAML)
            .map_err(|e| anyhow::anyhow!("failed to parse baked-in config.yaml: {e}"))
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

    #[test]
    fn model_overrides_resolve_per_key_then_global() {
        let yaml = r#"
version: 1
model_overrides:
  claude-fable-5: claude-opus-4-8
  gpt-4o: gpt-5.4
keys:
  - name: tldr-bot
    model_overrides:
      gpt-4o: gpt-5.4-mini
"#;
        let file: FileConfig = serde_yaml::from_str(yaml).unwrap();
        let config = Config {
            keys: file.keys,
            model_overrides: file.model_overrides,
            ..Default::default()
        };

        // Per-key entry wins over the global map.
        assert_eq!(config.override_model("tldr-bot", "gpt-4o"), Some("gpt-5.4-mini"));
        // Falls back to the global map when the key has no entry.
        assert_eq!(
            config.override_model("tldr-bot", "claude-fable-5"),
            Some("claude-opus-4-8")
        );
        // Unknown key still gets the global map.
        assert_eq!(config.override_model("other", "gpt-4o"), Some("gpt-5.4"));
        // No override configured.
        assert_eq!(config.override_model("tldr-bot", "claude-sonnet-4-6"), None);
    }
}
