use anyhow::{Context, anyhow};
use serde_json::Value;

pub const COMMITTED_AT_LABEL: &str = "net.inf-k8s.committed-at";

const MANIFEST_ACCEPT: &str = "application/vnd.oci.image.index.v1+json, \
application/vnd.docker.distribution.manifest.list.v2+json, \
application/vnd.oci.image.manifest.v1+json, \
application/vnd.docker.distribution.manifest.v2+json";

pub struct RegistryClient {
    client: reqwest::Client,
    base_url: String,
    token: Option<String>,
}

impl RegistryClient {
    pub fn from_env() -> anyhow::Result<Self> {
        let base_url =
            std::env::var("REGISTRY_URL").unwrap_or_else(|_| "https://ghcr.io".to_string());
        let token = std::env::var("GITHUB_TOKEN").ok();
        Ok(Self::new(base_url, token))
    }

    pub fn new(base_url: String, token: Option<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url: base_url.trim_end_matches('/').to_owned(),
            token,
        }
    }

    #[tracing::instrument(skip(self), err)]
    pub async fn committed_at(&self, image_ref: &str, tag: &str) -> anyhow::Result<i64> {
        let repo = image_ref
            .split_once('/')
            .map(|(_, rest)| rest)
            .unwrap_or(image_ref);

        let pull_token = self.fetch_pull_token(repo).await?;
        let config_digest = self.config_digest(repo, tag, &pull_token).await?;
        let config = self.fetch_blob(repo, &config_digest, &pull_token).await?;

        let raw = config
            .pointer("/config/Labels")
            .and_then(Value::as_object)
            .and_then(|labels| labels.get(COMMITTED_AT_LABEL))
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("image {image_ref}:{tag} missing {COMMITTED_AT_LABEL} label"))?;

        raw.parse::<i64>()
            .with_context(|| format!("{COMMITTED_AT_LABEL}={raw:?} is not an integer"))
    }

    async fn fetch_pull_token(&self, repo: &str) -> anyhow::Result<String> {
        let url = format!(
            "{}/token?service=ghcr.io&scope=repository:{repo}:pull",
            self.base_url
        );

        let mut req = self.client.get(&url).header("User-Agent", "janitor-bot");
        if let Some(token) = &self.token {
            req = req.bearer_auth(token);
        }

        let body: Value = req.send().await?.error_for_status()?.json().await?;
        body.get("token")
            .or_else(|| body.get("access_token"))
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| anyhow!("registry token response missing token field"))
    }

    async fn config_digest(
        &self,
        repo: &str,
        reference: &str,
        token: &str,
    ) -> anyhow::Result<String> {
        let manifest = self.fetch_manifest(repo, reference, token).await?;

        if let Some(manifests) = manifest.get("manifests").and_then(Value::as_array) {
            let digest = manifests
                .iter()
                .find(|m| {
                    m.pointer("/platform/architecture").and_then(Value::as_str) == Some("amd64")
                        && m.pointer("/platform/os").and_then(Value::as_str) == Some("linux")
                })
                .or_else(|| manifests.first())
                .and_then(|m| m.get("digest"))
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    anyhow!("manifest index for {repo}:{reference} had no usable entry")
                })?
                .to_owned();

            let image_manifest = self.fetch_manifest(repo, &digest, token).await?;
            return Self::config_digest_from_image_manifest(&image_manifest, repo, reference);
        }

        Self::config_digest_from_image_manifest(&manifest, repo, reference)
    }

    fn config_digest_from_image_manifest(
        manifest: &Value,
        repo: &str,
        reference: &str,
    ) -> anyhow::Result<String> {
        manifest
            .pointer("/config/digest")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| anyhow!("image manifest for {repo}:{reference} missing config digest"))
    }

    async fn fetch_manifest(
        &self,
        repo: &str,
        reference: &str,
        token: &str,
    ) -> anyhow::Result<Value> {
        let url = format!("{}/v2/{repo}/manifests/{reference}", self.base_url);
        let body = self
            .client
            .get(&url)
            .bearer_auth(token)
            .header("Accept", MANIFEST_ACCEPT)
            .header("User-Agent", "janitor-bot")
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(body)
    }

    async fn fetch_blob(&self, repo: &str, digest: &str, token: &str) -> anyhow::Result<Value> {
        let url = format!("{}/v2/{repo}/blobs/{digest}", self.base_url);
        let body = self
            .client
            .get(&url)
            .bearer_auth(token)
            .header("User-Agent", "janitor-bot")
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(body)
    }
}
