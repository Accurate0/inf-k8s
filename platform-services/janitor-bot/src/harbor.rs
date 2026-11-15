use anyhow::{Context, anyhow, bail};
use registry_api::oci::{Client, Credentials};
use serde_json::Value;

const REVISION: &str = "org.opencontainers.image.revision";

pub struct HarborClient {
    http: reqwest::Client,
    base: Option<String>,
    credentials: Option<Credentials>,
}

impl HarborClient {
    pub fn from_env() -> Self {
        let credentials = match (
            std::env::var("HARBOR_USERNAME"),
            std::env::var("HARBOR_PASSWORD"),
        ) {
            (Ok(username), Ok(password)) => Some(Credentials { username, password }),
            _ => None,
        };

        Self::new(None, credentials)
    }

    pub fn new(base: Option<String>, credentials: Option<Credentials>) -> Self {
        Self {
            http: reqwest::Client::new(),
            base,
            credentials,
        }
    }

    #[tracing::instrument(skip(self), err)]
    pub async fn commit(&self, artifact: &str, digest: &str) -> anyhow::Result<String> {
        let (host, repository) = artifact
            .strip_prefix("oci://")
            .and_then(|rest| rest.split_once('/'))
            .ok_or_else(|| anyhow!("{artifact} is not oci://<host>/<repository>"))?;

        let base = self
            .base
            .clone()
            .unwrap_or_else(|| format!("https://{host}"));
        let client = Client::new(self.http.clone(), &base, self.credentials.clone())?;
        let fetched = client
            .manifest(repository, digest)
            .await?
            .ok_or_else(|| anyhow!("{artifact}@{digest} does not exist"))?;

        let manifest: Value =
            serde_json::from_slice(&fetched.body).context("unreadable artifact manifest")?;
        manifest["annotations"][REVISION]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| anyhow!("{artifact}@{digest} has no {REVISION} annotation"))
    }

    pub async fn sync_commit(&self, sha: &str, artifact: &str) -> anyhow::Result<String> {
        if !sha.starts_with("sha256:") {
            return Ok(sha.to_owned());
        }
        if artifact.is_empty() {
            bail!("sync revision {sha} is an artifact digest but the event names no artifact");
        }

        self.commit(artifact, sha).await
    }
}
