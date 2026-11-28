use std::sync::Arc;
use std::time::Duration;

use moka::future::Cache;
use serde::Deserialize;

#[derive(Debug, Default, Deserialize)]
pub struct UserInfo {
    #[serde(default)]
    pub groups: Vec<String>,

    #[serde(default)]
    pub preferred_username: Option<String>,
}

pub struct UserInfoCache {
    client: reqwest::Client,
    cache: Cache<String, Arc<UserInfo>>,
}

impl UserInfoCache {
    pub fn new(client: reqwest::Client) -> Self {
        Self {
            client,
            cache: Cache::builder()
                .max_capacity(10_000)
                .time_to_live(Duration::from_secs(300))
                .build(),
        }
    }

    pub async fn get(&self, issuer: &str, token: &str) -> Arc<UserInfo> {
        self.cache
            .try_get_with(token.to_owned(), async {
                self.fetch(issuer, token).await.map(Arc::new)
            })
            .await
            .unwrap_or_else(|error| {
                tracing::warn!(issuer, "userinfo lookup failed: {error}");
                Arc::default()
            })
    }

    async fn fetch(&self, issuer: &str, token: &str) -> Result<UserInfo, reqwest::Error> {
        self.client
            .get(format!("{}/userinfo", issuer.trim_end_matches('/')))
            .bearer_auth(token)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await
    }
}
