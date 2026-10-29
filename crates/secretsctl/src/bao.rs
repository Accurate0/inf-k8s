use std::time::Duration;

use anyhow::{Context, bail};
use reqwest::{Method, StatusCode};
use serde::Deserialize;
use serde_json::{Map, Value, json};

pub struct Bao {
    addr: String,
    mount: String,
    token: String,
    http: reqwest::Client,
}

#[derive(Debug, Deserialize)]
pub struct Secret {
    pub data: Map<String, Value>,
    pub metadata: VersionMeta,
}

#[derive(Debug, Deserialize)]
pub struct VersionMeta {
    pub version: u64,
    #[serde(default)]
    pub created_time: String,
}

#[derive(Debug, Deserialize)]
pub struct Metadata {
    pub current_version: u64,
    #[serde(default)]
    pub versions: std::collections::BTreeMap<String, Version>,
}

#[derive(Debug, Deserialize)]
pub struct Version {
    pub created_time: String,
    #[serde(default)]
    pub deletion_time: String,
    #[serde(default)]
    pub destroyed: bool,
}

#[derive(Debug, Deserialize)]
pub struct TokenInfo {
    pub display_name: String,
    #[serde(default)]
    pub policies: Vec<String>,
    pub ttl: u64,
}

fn http() -> anyhow::Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .user_agent(concat!("secretsctl/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .build()?)
}

async fn error(response: reqwest::Response) -> anyhow::Error {
    #[derive(Deserialize)]
    struct Errors {
        #[serde(default)]
        errors: Vec<String>,
    }

    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    let detail = serde_json::from_str::<Errors>(&body)
        .ok()
        .filter(|e| !e.errors.is_empty())
        .map(|e| e.errors.join("; "))
        .unwrap_or(body);
    anyhow::anyhow!("openbao returned {status}: {}", detail.trim())
}

impl Bao {
    pub async fn login(addr: &str, mount: &str, jwt: &str) -> anyhow::Result<Self> {
        #[derive(Deserialize)]
        struct Login {
            auth: Auth,
        }
        #[derive(Deserialize)]
        struct Auth {
            client_token: String,
        }

        let addr = addr.trim_end_matches('/').to_owned();
        let http = http()?;
        let response = http
            .post(format!("{addr}/v1/auth/jwt/login"))
            .json(&json!({ "role": "cli", "jwt": jwt }))
            .send()
            .await
            .with_context(|| format!("logging in to {addr}"))?;
        if !response.status().is_success() {
            return Err(error(response).await.context("openbao jwt login failed"));
        }
        let login: Login = response.json().await?;
        Ok(Self::with_token(&addr, mount, login.auth.client_token, http))
    }

    pub fn from_token(addr: &str, mount: &str, token: String) -> anyhow::Result<Self> {
        Ok(Self::with_token(addr.trim_end_matches('/'), mount, token, http()?))
    }

    fn with_token(addr: &str, mount: &str, token: String, http: reqwest::Client) -> Self {
        Self {
            addr: addr.to_owned(),
            mount: mount.trim_matches('/').to_owned(),
            token,
            http,
        }
    }

    async fn call(
        &self,
        method: Method,
        path: &str,
        body: Option<(&str, Value)>,
    ) -> anyhow::Result<Option<Value>> {
        let mut request = self
            .http
            .request(method, format!("{}/v1/{path}", self.addr))
            .header("X-Vault-Token", &self.token);
        if let Some((content_type, body)) = body {
            request = request
                .header(reqwest::header::CONTENT_TYPE, content_type)
                .body(serde_json::to_vec(&body)?);
        }
        let response = request.send().await.context("calling openbao")?;
        match response.status() {
            StatusCode::NOT_FOUND => Ok(None),
            StatusCode::NO_CONTENT => Ok(Some(Value::Null)),
            status if status.is_success() => Ok(Some(response.json().await?)),
            _ => Err(error(response).await),
        }
    }

    fn data_of<T: serde::de::DeserializeOwned>(value: Value) -> anyhow::Result<T> {
        Ok(serde_json::from_value(
            value.get("data").cloned().context("openbao response has no data")?,
        )?)
    }

    pub async fn whoami(&self) -> anyhow::Result<TokenInfo> {
        let value = self
            .call(Method::GET, "auth/token/lookup-self", None)
            .await?
            .context("token lookup returned nothing")?;
        Self::data_of(value)
    }

    pub async fn list(&self, prefix: &str) -> anyhow::Result<Vec<String>> {
        #[derive(Deserialize)]
        struct Keys {
            keys: Vec<String>,
        }

        let prefix = prefix.trim_matches('/');
        let path = format!("{}/metadata/{prefix}", self.mount);
        let method = Method::from_bytes(b"LIST")?;
        match self.call(method, path.trim_end_matches('/'), None).await? {
            Some(value) => Ok(Self::data_of::<Keys>(value)?.keys),
            None => Ok(Vec::new()),
        }
    }

    pub async fn get(&self, path: &str) -> anyhow::Result<Option<Secret>> {
        let path = format!("{}/data/{}", self.mount, path.trim_matches('/'));
        match self.call(Method::GET, &path, None).await? {
            Some(value) => Ok(Some(Self::data_of(value)?)),
            None => Ok(None),
        }
    }

    pub async fn require(&self, path: &str) -> anyhow::Result<Secret> {
        self.get(path)
            .await?
            .with_context(|| format!("no secret at {}/{path}", self.mount))
    }

    pub async fn patch(&self, path: &str, changes: Map<String, Value>) -> anyhow::Result<u64> {
        let api = format!("{}/data/{}", self.mount, path.trim_matches('/'));
        let body = json!({ "data": changes });
        let written = match self
            .call(
                Method::PATCH,
                &api,
                Some(("application/merge-patch+json", body.clone())),
            )
            .await?
        {
            Some(value) => value,
            None => {
                let created: Map<String, Value> = changes
                    .into_iter()
                    .filter(|(_, v)| !v.is_null())
                    .collect();
                if created.is_empty() {
                    bail!("no secret at {}/{path}", self.mount);
                }
                self.call(
                    Method::POST,
                    &api,
                    Some(("application/json", json!({ "data": created }))),
                )
                .await?
                .context("openbao did not create the secret")?
            }
        };
        Ok(written
            .pointer("/data/version")
            .and_then(Value::as_u64)
            .unwrap_or_default())
    }

    pub async fn delete_latest(&self, path: &str) -> anyhow::Result<()> {
        let api = format!("{}/data/{}", self.mount, path.trim_matches('/'));
        self.call(Method::DELETE, &api, None).await?;
        Ok(())
    }

    pub async fn metadata(&self, path: &str) -> anyhow::Result<Metadata> {
        let api = format!("{}/metadata/{}", self.mount, path.trim_matches('/'));
        let value = self
            .call(Method::GET, &api, None)
            .await?
            .with_context(|| format!("no secret at {}/{path}", self.mount))?;
        Self::data_of(value)
    }
}
