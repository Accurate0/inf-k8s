use crate::config::Span;
use crate::error::Result;
use ipnet::IpNet;
use k8s_openapi::api::core::v1::ConfigMap;
use kube::Client;
use kube::api::{Api, DeleteParams};
use serde::{Deserialize, Serialize};
use sqlx::postgres::PgPool;

const CONFIGMAP: &str = "waf-manager-suppressions";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Entry {
    cidr: String,
    until: String,
}

pub struct Suppressions {
    pool: PgPool,
    api: Api<ConfigMap>,
    cooldown: Span,
}

impl Suppressions {
    pub fn new(pool: PgPool, client: Client, namespace: &str, cooldown: Span) -> Self {
        Self {
            pool,
            api: Api::namespaced(client, namespace),
            cooldown,
        }
    }

    pub async fn active(&self, now: chrono::DateTime<chrono::Utc>) -> Result<Vec<IpNet>> {
        let rows = sqlx::query!("select cidr from suppressions where until > $1", now)
            .fetch_all(&self.pool)
            .await?;

        Ok(rows
            .into_iter()
            .filter_map(|row| row.cidr.parse::<IpNet>().ok())
            .collect())
    }

    pub async fn record(&self, net: &IpNet, now: chrono::DateTime<chrono::Utc>) -> Result<()> {
        let until =
            now + chrono::Duration::from_std(self.cooldown.0).unwrap_or(chrono::Duration::zero());

        self.upsert(&net.to_string(), until).await
    }

    pub async fn prune(&self, now: chrono::DateTime<chrono::Utc>) -> Result<()> {
        sqlx::query!("delete from suppressions where until <= $1", now)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    pub async fn import_configmap(&self, now: chrono::DateTime<chrono::Utc>) -> Result<()> {
        let Some(cm) = self.api.get_opt(CONFIGMAP).await? else {
            return Ok(());
        };

        let entries: Vec<Entry> = cm
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
            .filter(|entry| Self::is_live(entry, now))
            .collect();

        for entry in &entries {
            let until = match chrono::DateTime::parse_from_rfc3339(&entry.until) {
                Ok(until) => until.with_timezone(&chrono::Utc),
                Err(_) => {
                    now + chrono::Duration::from_std(self.cooldown.0)
                        .unwrap_or(chrono::Duration::zero())
                }
            };

            self.upsert(&entry.cidr, until).await?;
        }

        self.api.delete(CONFIGMAP, &DeleteParams::default()).await?;

        tracing::info!("imported {} suppressions from {CONFIGMAP}", entries.len());
        Ok(())
    }

    async fn upsert(&self, cidr: &str, until: chrono::DateTime<chrono::Utc>) -> Result<()> {
        sqlx::query!(
            "insert into suppressions (cidr, until) values ($1, $2)
             on conflict (cidr) do update set until = excluded.until",
            cidr,
            until,
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    fn is_live(entry: &Entry, now: chrono::DateTime<chrono::Utc>) -> bool {
        match chrono::DateTime::parse_from_rfc3339(&entry.until) {
            Ok(until) => until.with_timezone(&chrono::Utc) > now,
            Err(_) => true,
        }
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
}
