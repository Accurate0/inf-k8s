use std::time::Duration;

use chrono::{DateTime, Utc};
use registry_api::{HarborAccess, HarborAction};
use reqwest::StatusCode;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{config::HarborConfig, error::ApiError};

#[derive(Clone)]
pub struct Harbor {
    http: reqwest::Client,
    url: String,
    username: String,
    password: String,
}

#[derive(Debug, Deserialize)]
pub struct CreatedRobot {
    pub id: i64,
    pub name: String,
    pub secret: String,
    #[serde(default)]
    pub expires_at: Option<i64>,
}

impl CreatedRobot {
    pub fn expiry(&self, fallback: DateTime<Utc>) -> DateTime<Utc> {
        self.expires_at
            .and_then(|at| DateTime::from_timestamp(at, 0))
            .unwrap_or(fallback)
    }
}

fn permissions(access: &[HarborAccess]) -> Value {
    access
        .iter()
        .map(|item| {
            let mut actions = vec![HarborAction::Pull];
            if item.actions.contains(&HarborAction::Push) {
                actions.push(HarborAction::Push);
            }

            json!({
                "kind": "project",
                "namespace": item.project,
                "access": actions
                    .iter()
                    .map(|action| json!({ "resource": "repository", "action": action }))
                    .collect::<Vec<_>>(),
            })
        })
        .collect()
}

impl Harbor {
    pub fn new(url: &str, username: String, password: String) -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(20))
                .user_agent("registry")
                .build()
                .unwrap_or_default(),
            url: url.trim_end_matches('/').to_owned(),
            username,
            password,
        }
    }

    pub fn from_env(config: &HarborConfig) -> Option<Self> {
        let (Ok(username), Ok(password)) = (
            std::env::var("HARBOR_USERNAME"),
            std::env::var("HARBOR_PASSWORD"),
        ) else {
            tracing::warn!("HARBOR_USERNAME/HARBOR_PASSWORD unset; harbor token exchange disabled");
            return None;
        };
        Some(Self::new(&config.url, username, password))
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        self.http
            .request(method, format!("{}/api/v2.0{path}", self.url))
            .basic_auth(&self.username, Some(&self.password))
    }

    pub async fn create_robot(
        &self,
        name: &str,
        description: &str,
        duration_days: i64,
        access: &[HarborAccess],
    ) -> Result<CreatedRobot, ApiError> {
        let response = self
            .request(reqwest::Method::POST, "/robots")
            .json(&json!({
                "name": name,
                "description": description,
                "duration": duration_days,
                "level": "system",
                "disable": false,
                "permissions": permissions(access),
            }))
            .send()
            .await
            .map_err(|e| ApiError::Harbor(format!("creating robot: {e}")))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(ApiError::Harbor(format!(
                "creating robot failed ({status}): {body}"
            )));
        }

        response
            .json()
            .await
            .map_err(|e| ApiError::Harbor(format!("reading created robot: {e}")))
    }

    pub async fn delete_robot(&self, id: i64) -> Result<(), ApiError> {
        let response = self
            .request(reqwest::Method::DELETE, &format!("/robots/{id}"))
            .send()
            .await
            .map_err(|e| ApiError::Harbor(format!("deleting robot {id}: {e}")))?;

        let status = response.status();
        if status.is_success() || status == StatusCode::NOT_FOUND {
            return Ok(());
        }
        let body = response.text().await.unwrap_or_default();
        Err(ApiError::Harbor(format!(
            "deleting robot {id} failed ({status}): {body}"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_implies_pull() {
        let permissions = permissions(&[HarborAccess {
            project: "containers".to_owned(),
            actions: vec![HarborAction::Push],
            package: None,
        }]);
        assert_eq!(
            permissions,
            json!([{
                "kind": "project",
                "namespace": "containers",
                "access": [
                    { "resource": "repository", "action": "pull" },
                    { "resource": "repository", "action": "push" },
                ],
            }])
        );
    }
}
