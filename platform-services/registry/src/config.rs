use serde::Deserialize;

use crate::auth::grants::Grant;

const CONFIG_YAML: &str = include_str!("../config.yaml");

const CONFIG_PATH_ENV: &str = "CONFIG_PATH";

pub const CONFIG_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub version: u32,
    pub oauth: OAuthConfig,
    #[serde(default)]
    pub forgejo: Option<ForgejoConfig>,
    pub storage: StorageConfig,
    #[serde(default)]
    pub limits: Limits,
    #[serde(default)]
    pub graph: Option<GraphConfig>,
    #[serde(default)]
    pub harbor: Option<HarborConfig>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct HarborConfig {
    pub url: String,
    pub registry: String,
    pub packages_project: String,
    #[serde(default = "default_robot_duration_days")]
    pub robot_duration_days: i64,
    #[serde(default)]
    pub grants: Vec<Grant>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct GraphConfig {
    pub issuer: String,
    pub audience: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct OAuthConfig {
    pub issuer: String,
    #[serde(default)]
    pub audience: Option<String>,
    #[serde(default)]
    pub jwks_uri: Option<String>,
    #[serde(default)]
    pub userinfo_endpoint: Option<String>,
    #[serde(default)]
    pub read_groups: Vec<String>,
    #[serde(default)]
    pub admin_groups: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ForgejoConfig {
    pub issuer: String,
    pub audiences: Vec<String>,
    #[serde(default)]
    pub jwks_uri: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct StorageConfig {
    pub bucket: String,
    #[serde(default = "default_region")]
    pub region: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Limits {
    #[serde(default = "default_max_artifact_bytes")]
    pub max_artifact_bytes: u64,
    #[serde(default = "default_draft_ttl_hours")]
    pub draft_ttl_hours: i64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_artifact_bytes: default_max_artifact_bytes(),
            draft_ttl_hours: default_draft_ttl_hours(),
        }
    }
}

fn default_region() -> String {
    "auto".to_owned()
}

fn default_max_artifact_bytes() -> u64 {
    256 * 1024 * 1024
}

fn default_draft_ttl_hours() -> i64 {
    24
}

fn default_robot_duration_days() -> i64 {
    1
}

impl Config {
    pub fn load() -> anyhow::Result<Self> {
        let Ok(path) = std::env::var(CONFIG_PATH_ENV) else {
            tracing::info!("{CONFIG_PATH_ENV} unset; using baked-in config");
            return Self::baked_in();
        };

        let contents = std::fs::read_to_string(&path)
            .map_err(|e| anyhow::anyhow!("failed to read config ConfigMap at {path}: {e}"))?;
        let config: Config = serde_yaml::from_str(&contents)
            .map_err(|e| anyhow::anyhow!("failed to parse config ConfigMap at {path}: {e}"))?;
        config.validate()?;

        if config.version != CONFIG_SCHEMA_VERSION {
            tracing::warn!(
                configmap_version = config.version,
                code_version = CONFIG_SCHEMA_VERSION,
                "config ConfigMap version incompatible with this binary; using baked-in config"
            );
            return Self::baked_in();
        }

        tracing::info!(
            path,
            version = config.version,
            "loaded config from ConfigMap"
        );
        Ok(config)
    }

    pub fn baked_in() -> anyhow::Result<Self> {
        let config: Config = serde_yaml::from_str(CONFIG_YAML)
            .map_err(|e| anyhow::anyhow!("failed to parse baked-in config.yaml: {e}"))?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> anyhow::Result<()> {
        let Some(harbor) = &self.harbor else {
            return Ok(());
        };

        if harbor.robot_duration_days < 1 {
            anyhow::bail!("harbor.robot_duration_days must be at least 1");
        }
        for (i, grant) in harbor.grants.iter().enumerate() {
            grant
                .validate()
                .map_err(|e| anyhow::anyhow!("harbor.grants[{i}]: {e}"))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_config_parses() {
        let config = Config::baked_in().unwrap();
        assert_eq!(config.version, CONFIG_SCHEMA_VERSION);
        assert_eq!(config.storage.bucket, "registry");
        assert!(config.forgejo.is_some());
        assert!(config.graph.is_some());
        assert!(!config.harbor.unwrap().grants.is_empty());
    }
}
