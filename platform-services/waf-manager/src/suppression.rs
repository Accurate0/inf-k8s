use crate::config::Span;
use crate::error::Result;
use ipnet::IpNet;
use k8s_openapi::api::core::v1::ConfigMap;
use kube::Client;
use kube::api::{Api, Patch, PatchParams};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const CONFIGMAP: &str = "waf-manager-suppressions";
const FIELD_MANAGER: &str = "waf-manager";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Entry {
    cidr: String,
    until: String,
}

/// CIDRs workflows must leave alone, because a human took the block off.
pub struct Suppressions {
    api: Api<ConfigMap>,
    cooldown: Span,
}

impl Suppressions {
    pub fn new(client: Client, namespace: &str, cooldown: Span) -> Self {
        Self {
            api: Api::namespaced(client, namespace),
            cooldown,
        }
    }

    pub async fn active(&self, now: chrono::DateTime<chrono::Utc>) -> Result<Vec<IpNet>> {
        Ok(self
            .entries(now)
            .await?
            .into_iter()
            .filter_map(|e| e.cidr.parse::<IpNet>().ok())
            .collect())
    }

    pub async fn record(&self, net: &IpNet, now: chrono::DateTime<chrono::Utc>) -> Result<()> {
        let until =
            now + chrono::Duration::from_std(self.cooldown.0).unwrap_or(chrono::Duration::zero());

        let mut entries = self.entries(now).await?;
        entries.retain(|e| e.cidr != net.to_string());
        entries.push(Entry {
            cidr: net.to_string(),
            until: until.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        });

        self.write(entries).await
    }

    /// Keeps the ConfigMap from growing without bound.
    pub async fn prune(&self, now: chrono::DateTime<chrono::Utc>) -> Result<()> {
        let before = self.raw().await?.len();
        let entries = self.entries(now).await?;

        if entries.len() == before {
            return Ok(());
        }

        self.write(entries).await
    }

    async fn entries(&self, now: chrono::DateTime<chrono::Utc>) -> Result<Vec<Entry>> {
        Ok(self
            .raw()
            .await?
            .into_iter()
            .filter(|e| Self::is_live(e, now))
            .collect())
    }

    async fn raw(&self) -> Result<Vec<Entry>> {
        let Some(cm) = self.api.get_opt(CONFIGMAP).await? else {
            return Ok(Vec::new());
        };

        Ok(cm
            .data
            .unwrap_or_default()
            .into_values()
            .filter_map(|raw| match serde_json::from_str::<Entry>(&raw) {
                Ok(entry) => Some(entry),
                Err(e) => {
                    tracing::warn!("dropping unparsable suppression {raw:?}: {e}");
                    None
                }
            })
            .collect())
    }

    async fn write(&self, entries: Vec<Entry>) -> Result<()> {
        let data: BTreeMap<String, String> = entries
            .iter()
            .filter_map(|e| {
                let value = serde_json::to_string(e).ok()?;
                Some((Self::key(&e.cidr), value))
            })
            .collect();

        // Apply, not patch: a pruned entry has to disappear rather than linger
        // as an unmentioned key.
        let cm = serde_json::json!({
            "apiVersion": "v1",
            "kind": "ConfigMap",
            "metadata": { "name": CONFIGMAP },
            "data": data,
        });

        self.api
            .patch(
                CONFIGMAP,
                &PatchParams::apply(FIELD_MANAGER).force(),
                &Patch::Apply(&cm),
            )
            .await?;

        Ok(())
    }

    /// An unparsable timestamp counts as live; the cost is only a block that is
    /// not re-created.
    fn is_live(entry: &Entry, now: chrono::DateTime<chrono::Utc>) -> bool {
        match chrono::DateTime::parse_from_rfc3339(&entry.until) {
            Ok(until) => until.with_timezone(&chrono::Utc) > now,
            Err(_) => true,
        }
    }

    /// ConfigMap keys allow only `[-._a-zA-Z0-9]`, ruling out `/` and `:`. The
    /// CIDR is kept in the value, so the key only has to be unique.
    fn key(cidr: &str) -> String {
        let mut key = String::with_capacity(cidr.len());
        let mut last_dash = true;

        for ch in cidr.chars() {
            if ch.is_ascii_alphanumeric() {
                key.push(ch.to_ascii_lowercase());
                last_dash = false;
            } else if !last_dash {
                key.push('-');
                last_dash = true;
            }
        }

        key.trim_matches('-').to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(raw: &str) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339(raw)
            .unwrap()
            .with_timezone(&chrono::Utc)
    }

    #[test]
    fn expired_entries_are_not_live() {
        let entry = Entry {
            cidr: "203.0.113.4/32".to_string(),
            until: "2026-01-01T00:00:00Z".to_string(),
        };

        assert!(Suppressions::is_live(&entry, at("2025-12-31T23:59:59Z")));
        assert!(!Suppressions::is_live(&entry, at("2026-01-01T00:00:01Z")));
    }

    #[test]
    fn an_unparsable_expiry_keeps_the_suppression() {
        let entry = Entry {
            cidr: "203.0.113.4/32".to_string(),
            until: "whenever".to_string(),
        };

        assert!(Suppressions::is_live(&entry, at("2026-01-01T00:00:00Z")));
    }

    #[test]
    fn keys_are_valid_configmap_keys() {
        assert_eq!(Suppressions::key("203.0.113.4/32"), "203-0-113-4-32");
        assert_eq!(Suppressions::key("2606:4700::1/128"), "2606-4700-1-128");
    }
}
