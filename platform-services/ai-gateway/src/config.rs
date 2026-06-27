use std::collections::{BTreeMap, HashMap};

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
pub const CONFIG_SCHEMA_VERSION: u32 = 2;

#[derive(Clone, Debug, Default)]
pub struct Config {
    pub admin_token: String,
    pub providers: HashMap<String, ProviderConfig>,
    pub keys: Vec<KeyConfig>,
    /// Ordered model-resolution rules, evaluated first-match-wins per request. Subsumes
    /// global/per-key overrides, provider reroutes, and model denial. See [`Config::resolve`].
    pub rules: Vec<Rule>,
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
    rules: Vec<Rule>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ProviderConfig {
    pub dialect: Dialect,
    pub base_url: String,
    #[serde(default)]
    pub api_key_env: Option<String>,
    /// Chat model ids this provider actually serves.
    #[serde(default)]
    pub models: Vec<String>,
    /// Embedding model ids this provider actually serves.
    #[serde(default)]
    pub embedding_models: Vec<String>,
    /// Failover order among providers that serve the same model: lower is tried first.
    #[serde(default = "default_priority")]
    pub priority: i32,
    /// When set, this provider serves any model that no provider explicitly declares,
    /// passing the requested model through unchanged. Multiple fallbacks are tried in
    /// `priority` order.
    #[serde(default)]
    pub fallback: bool,
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

/// One model-resolution rule: when `match` matches the request, its action applies and
/// evaluation stops. Exactly one action should be set; if several are, `deny` wins, then
/// `route`, then `set_model`.
///
/// ```yaml
/// rules:
///   - match:                  # per-key override
///       key: tldr-bot
///       model: gpt-4o
///     set_model: gpt-5.4-mini
///   - match:                  # global override
///       model: gpt-4o
///     set_model: gpt-5.4
///   - match:                  # same-provider reroute
///       model: claude-fable-5
///     route:
///       provider: anthropic
///       as: claude-opus-4-8
///   - match:                  # deny
///       model: claude-haiku-4-5
///     deny: true
/// ```
#[derive(Clone, Debug, Default, Deserialize)]
pub struct Rule {
    #[serde(default, rename = "match")]
    pub matcher: RuleMatch,
    #[serde(default)]
    pub set_model: Option<String>,
    #[serde(default)]
    pub route: Option<RouteAction>,
    #[serde(default)]
    pub deny: bool,
}

/// Conditions a rule matches on; absent fields match anything, all present fields must
/// match (AND). An empty matcher is a catch-all.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct RuleMatch {
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
}

impl RuleMatch {
    fn matches(&self, key: &str, model: &str) -> bool {
        self.key.as_deref().is_none_or(|k| k == key)
            && self.model.as_deref().is_none_or(|m| m == model)
    }
}

/// Pins the request to a specific provider, sending it `as` (defaulting to the requested
/// model). Unlike `set_model`, routing does not fall through to other providers.
#[derive(Clone, Debug, Deserialize)]
pub struct RouteAction {
    pub provider: String,
    #[serde(default, rename = "as")]
    pub as_model: Option<String>,
}

/// Outcome of resolving a request against the rules.
#[derive(Debug, PartialEq, Eq)]
pub enum Resolved {
    /// Route to `model`, optionally pinned to a single `provider`.
    Route {
        model: String,
        provider: Option<String>,
    },
    /// A `deny` rule matched; the request is rejected.
    Denied,
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
            rules: file.rules,
        })
    }

    /// Adjusts the provider-served `models` map (model id -> owner) by the globally-scoped
    /// rules (those without a `key` condition), for advertising via `/v1/models`: a `deny`
    /// removes a model, and a keyless `route`/`set_model` adds the request-facing id it
    /// matches on (owned by the route's provider, or the target's owner for `set_model`).
    pub fn advertise(&self, models: &mut BTreeMap<String, String>) {
        for rule in &self.rules {
            if rule.matcher.key.is_some() {
                continue;
            }
            let Some(model) = rule.matcher.model.clone() else {
                continue;
            };
            if rule.deny {
                models.remove(&model);
            } else if let Some(route) = &rule.route {
                models.insert(model, route.provider.clone());
            } else if let Some(target) = &rule.set_model {
                let owner = models.get(target).cloned().unwrap_or_default();
                models.insert(model, owner);
            }
        }
    }

    /// Resolves `requested` for `key_name` against the rules, first match wins. With no
    /// matching rule the request routes unchanged.
    pub fn resolve(&self, key_name: &str, requested: &str) -> Resolved {
        for rule in &self.rules {
            if !rule.matcher.matches(key_name, requested) {
                continue;
            }
            if rule.deny {
                return Resolved::Denied;
            }
            if let Some(route) = &rule.route {
                return Resolved::Route {
                    model: route
                        .as_model
                        .clone()
                        .unwrap_or_else(|| requested.to_owned()),
                    provider: Some(route.provider.clone()),
                };
            }
            if let Some(model) = &rule.set_model {
                return Resolved::Route {
                    model: model.clone(),
                    provider: None,
                };
            }
        }
        Resolved::Route {
            model: requested.to_owned(),
            provider: None,
        }
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

    fn config_from(yaml: &str) -> Config {
        let file: FileConfig = serde_yaml::from_str(yaml).unwrap();
        Config {
            keys: file.keys,
            rules: file.rules,
            ..Default::default()
        }
    }

    #[test]
    fn rules_resolve_first_match_wins() {
        let config = config_from(
            r#"
version: 2
rules:
  - match: { key: tldr-bot, model: gpt-4o }
    set_model: gpt-5.4-mini
  - match: { model: gpt-4o }
    set_model: gpt-5.4
"#,
        );

        // The more specific keyed rule precedes the global one, so it wins.
        assert_eq!(
            config.resolve("tldr-bot", "gpt-4o"),
            Resolved::Route {
                model: "gpt-5.4-mini".into(),
                provider: None
            }
        );
        // Other keys fall through to the global rule.
        assert_eq!(
            config.resolve("other", "gpt-4o"),
            Resolved::Route {
                model: "gpt-5.4".into(),
                provider: None
            }
        );
        // No rule matches: routes unchanged.
        assert_eq!(
            config.resolve("other", "claude-sonnet-4-6"),
            Resolved::Route {
                model: "claude-sonnet-4-6".into(),
                provider: None
            }
        );
    }

    #[test]
    fn route_rule_pins_provider_and_deny_rejects() {
        let config = config_from(
            r#"
version: 2
rules:
  - match: { model: claude-fable-5 }
    route: { provider: anthropic, as: claude-opus-4-8 }
  - match: { model: legacy-model }
    route: { provider: openrouter }
  - match: { model: blocked }
    deny: true
"#,
        );

        assert_eq!(
            config.resolve("any", "claude-fable-5"),
            Resolved::Route {
                model: "claude-opus-4-8".into(),
                provider: Some("anthropic".into())
            }
        );
        // `as` defaults to the requested model.
        assert_eq!(
            config.resolve("any", "legacy-model"),
            Resolved::Route {
                model: "legacy-model".into(),
                provider: Some("openrouter".into())
            }
        );
        assert_eq!(config.resolve("any", "blocked"), Resolved::Denied);
    }
}
