use std::{
    collections::{BTreeMap, HashMap},
    path::Path,
    sync::Mutex,
    time::{Duration, Instant},
};

use bytes::Bytes;
use futures::{Stream, TryStreamExt};
use reqwest::{Method, RequestBuilder, Response, StatusCode, header};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub const INDEX: &str = "application/vnd.oci.image.index.v1+json";
pub const MANIFEST: &str = "application/vnd.oci.image.manifest.v1+json";
pub const EMPTY: &str = "application/vnd.oci.empty.v1+json";
pub const EMPTY_JSON: &[u8] = b"{}";
pub const TITLE: &str = "org.opencontainers.image.title";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Http(#[from] reqwest::Error),
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{method} {url} failed ({status}): {body}")]
    Status {
        method: Method,
        url: String,
        status: StatusCode,
        body: String,
    },
    #[error("{0}")]
    Protocol(String),
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Platform {
    pub architecture: String,
    pub os: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Descriptor {
    pub media_type: String,
    pub digest: String,
    pub size: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<Platform>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub annotations: BTreeMap<String, String>,
}

impl Descriptor {
    pub fn new(media_type: &str, digest: String, size: u64) -> Self {
        Self {
            media_type: media_type.to_owned(),
            digest,
            size,
            artifact_type: None,
            platform: None,
            annotations: BTreeMap::new(),
        }
    }

    pub fn empty() -> Self {
        Self::new(EMPTY, digest(EMPTY_JSON), EMPTY_JSON.len() as u64)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub schema_version: u32,
    pub media_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_type: Option<String>,
    pub config: Descriptor,
    pub layers: Vec<Descriptor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<Descriptor>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub annotations: BTreeMap<String, String>,
}

impl Manifest {
    pub fn artifact(artifact_type: &str, layers: Vec<Descriptor>) -> Self {
        Self {
            schema_version: 2,
            media_type: MANIFEST.to_owned(),
            artifact_type: Some(artifact_type.to_owned()),
            config: Descriptor::empty(),
            layers,
            subject: None,
            annotations: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Index {
    pub schema_version: u32,
    pub media_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_type: Option<String>,
    pub manifests: Vec<Descriptor>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub annotations: BTreeMap<String, String>,
}

pub fn digest(bytes: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
}

pub fn sha256_digest(hex: &str) -> String {
    format!("sha256:{hex}")
}

pub struct Fetched {
    pub digest: String,
    pub media_type: String,
    pub body: Bytes,
}

#[derive(Debug, Clone)]
pub struct Credentials {
    pub username: String,
    pub password: String,
}

#[derive(Clone)]
enum Auth {
    Anonymous,
    Basic,
    Bearer(String),
}

struct Cached {
    auth: Auth,
    until: Instant,
}

pub struct Client {
    http: reqwest::Client,
    base: url::Url,
    credentials: Option<Credentials>,
    tokens: Mutex<HashMap<String, Cached>>,
    chunk_size: usize,
}

const TOKEN_TTL: Duration = Duration::from_secs(240);

fn check_digest(expected: &str, body: &[u8]) -> Result<()> {
    let actual = digest(body);
    if actual != expected {
        return Err(Error::Protocol(format!(
            "digest mismatch: expected {expected}, got {actual}"
        )));
    }
    Ok(())
}

fn challenge_param<'a>(challenge: &'a str, key: &str) -> Option<&'a str> {
    challenge.split(',').find_map(|part| {
        let (k, v) = part.trim().split_once('=')?;
        let k = k.rsplit(' ').next()?;
        (k.eq_ignore_ascii_case(key)).then(|| v.trim_matches('"'))
    })
}

impl Client {
    pub fn new(
        http: reqwest::Client,
        base: &str,
        credentials: Option<Credentials>,
    ) -> Result<Self> {
        let base = url::Url::parse(base)
            .map_err(|e| Error::Protocol(format!("invalid registry url {base:?}: {e}")))?;
        Ok(Self {
            http,
            base,
            credentials,
            tokens: Mutex::new(HashMap::new()),
            chunk_size: 32 * 1024 * 1024,
        })
    }

    pub fn base(&self) -> &str {
        self.base.as_str()
    }

    pub fn with_chunk_size(mut self, chunk_size: usize) -> Self {
        self.chunk_size = chunk_size;
        self
    }

    fn rebase(&self, location: &str) -> Result<url::Url> {
        let parsed = self
            .base
            .join(location)
            .map_err(|e| Error::Protocol(format!("invalid location {location:?}: {e}")))?;
        let mut url = self.base.clone();
        url.set_path(parsed.path());
        url.set_query(parsed.query());
        Ok(url)
    }

    fn url(&self, path: &str) -> Result<url::Url> {
        self.rebase(path)
    }

    async fn authorize(&self, repository: &str, push: bool) -> Result<Auth> {
        let scope = format!(
            "repository:{repository}:{}",
            if push { "pull,push" } else { "pull" }
        );
        if let Some(cached) = self.tokens.lock().unwrap().get(&scope)
            && cached.until > Instant::now()
        {
            return Ok(cached.auth.clone());
        }

        let ping = self.http.get(self.url("/v2/")?).send().await?;
        let auth = match ping.status() {
            status if status.is_success() => Auth::Anonymous,
            StatusCode::UNAUTHORIZED => {
                let challenge = ping
                    .headers()
                    .get(header::WWW_AUTHENTICATE)
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or_default()
                    .to_owned();
                if challenge.to_ascii_lowercase().starts_with("basic") {
                    Auth::Basic
                } else {
                    self.bearer(&challenge, &scope).await?
                }
            }
            status => {
                return Err(Error::Status {
                    method: Method::GET,
                    url: self.url("/v2/")?.to_string(),
                    status,
                    body: ping.text().await.unwrap_or_default(),
                });
            }
        };

        self.tokens.lock().unwrap().insert(
            scope,
            Cached {
                auth: auth.clone(),
                until: Instant::now() + TOKEN_TTL,
            },
        );
        Ok(auth)
    }

    async fn bearer(&self, challenge: &str, scope: &str) -> Result<Auth> {
        #[derive(Deserialize)]
        struct Token {
            #[serde(default)]
            token: Option<String>,
            #[serde(default)]
            access_token: Option<String>,
        }

        let realm = challenge_param(challenge, "realm")
            .ok_or_else(|| Error::Protocol(format!("no realm in challenge {challenge:?}")))?;
        let mut url = self.rebase(realm)?;
        {
            let mut query = url.query_pairs_mut();
            if let Some(service) = challenge_param(challenge, "service") {
                query.append_pair("service", service);
            }
            query.append_pair("scope", scope);
        }

        let mut request = self.http.get(url.clone());
        if let Some(credentials) = &self.credentials {
            request = request.basic_auth(&credentials.username, Some(&credentials.password));
        }
        let response = Self::ok(Method::GET, url.as_str(), request.send().await?).await?;
        let token: Token = response.json().await?;
        token
            .token
            .or(token.access_token)
            .map(Auth::Bearer)
            .ok_or_else(|| Error::Protocol("token response had no token".to_owned()))
    }

    async fn request(
        &self,
        method: Method,
        url: url::Url,
        repository: &str,
        push: bool,
    ) -> Result<RequestBuilder> {
        let request = self.http.request(method, url);
        Ok(match self.authorize(repository, push).await? {
            Auth::Anonymous => request,
            Auth::Basic => match &self.credentials {
                Some(c) => request.basic_auth(&c.username, Some(&c.password)),
                None => request,
            },
            Auth::Bearer(token) => request.bearer_auth(token),
        })
    }

    async fn ok(method: Method, url: &str, response: Response) -> Result<Response> {
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }
        Err(Error::Status {
            method,
            url: url.to_owned(),
            status,
            body: response.text().await.unwrap_or_default(),
        })
    }

    pub async fn manifest(&self, repository: &str, reference: &str) -> Result<Option<Fetched>> {
        let url = self.url(&format!("/v2/{repository}/manifests/{reference}"))?;
        let response = self
            .request(Method::GET, url.clone(), repository, false)
            .await?
            .header(header::ACCEPT, format!("{INDEX}, {MANIFEST}"))
            .send()
            .await?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let response = Self::ok(Method::GET, url.as_str(), response).await?;
        let media_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        let body = response.bytes().await?;
        if reference.starts_with("sha256:") {
            check_digest(reference, &body)?;
        }
        Ok(Some(Fetched {
            digest: digest(&body),
            media_type,
            body,
        }))
    }

    pub async fn put_manifest(
        &self,
        repository: &str,
        reference: &str,
        media_type: &str,
        body: &[u8],
    ) -> Result<String> {
        let url = self.url(&format!("/v2/{repository}/manifests/{reference}"))?;
        let response = self
            .request(Method::PUT, url.clone(), repository, true)
            .await?
            .header(header::CONTENT_TYPE, media_type)
            .body(body.to_vec())
            .send()
            .await?;
        Self::ok(Method::PUT, url.as_str(), response).await?;
        Ok(digest(body))
    }

    pub async fn blob_exists(&self, repository: &str, digest: &str) -> Result<bool> {
        let url = self.url(&format!("/v2/{repository}/blobs/{digest}"))?;
        let response = self
            .request(Method::HEAD, url.clone(), repository, false)
            .await?
            .send()
            .await?;
        match response.status() {
            StatusCode::NOT_FOUND => Ok(false),
            _ => Self::ok(Method::HEAD, url.as_str(), response)
                .await
                .map(|_| true),
        }
    }

    pub async fn blob(&self, repository: &str, digest: &str) -> Result<Response> {
        let url = self.url(&format!("/v2/{repository}/blobs/{digest}"))?;
        let response = self
            .request(Method::GET, url.clone(), repository, false)
            .await?
            .send()
            .await?;
        Self::ok(Method::GET, url.as_str(), response).await
    }

    pub async fn blob_bytes(&self, repository: &str, digest: &str, max: u64) -> Result<Bytes> {
        let response = self.blob(repository, digest).await?;
        if response.content_length().is_some_and(|len| len > max) {
            return Err(Error::Protocol(format!(
                "{digest} is larger than {max} bytes"
            )));
        }
        let body = response.bytes().await?;
        if body.len() as u64 > max {
            return Err(Error::Protocol(format!(
                "{digest} is larger than {max} bytes"
            )));
        }
        check_digest(digest, &body)?;
        Ok(body)
    }

    pub async fn blob_to_file(&self, repository: &str, digest: &str, dest: &Path) -> Result<u64> {
        let response = self.blob(repository, digest).await?;
        let mut file = tokio::fs::File::create(dest).await?;
        let mut hasher = Sha256::new();
        let mut size = 0;
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.try_next().await? {
            hasher.update(&chunk);
            size += chunk.len() as u64;
            file.write_all(&chunk).await?;
        }
        file.flush().await?;

        let actual = format!("sha256:{}", hex::encode(hasher.finalize()));
        if actual != digest {
            drop(file);
            let _ = tokio::fs::remove_file(dest).await;
            return Err(Error::Protocol(format!(
                "digest mismatch: expected {digest}, got {actual}"
            )));
        }
        Ok(size)
    }

    async fn start_upload(&self, repository: &str) -> Result<url::Url> {
        let url = self.url(&format!("/v2/{repository}/blobs/uploads/"))?;
        let response = self
            .request(Method::POST, url.clone(), repository, true)
            .await?
            .header(header::CONTENT_LENGTH, 0)
            .send()
            .await?;
        let response = Self::ok(Method::POST, url.as_str(), response).await?;
        self.location(&response)
    }

    fn location(&self, response: &Response) -> Result<url::Url> {
        let location = response
            .headers()
            .get(header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| Error::Protocol("upload response had no location".to_owned()))?;
        self.rebase(location)
    }

    async fn finish_upload(
        &self,
        repository: &str,
        mut location: url::Url,
        digest: &str,
        body: reqwest::Body,
        length: u64,
    ) -> Result<()> {
        location.query_pairs_mut().append_pair("digest", digest);
        let response = self
            .request(Method::PUT, location.clone(), repository, true)
            .await?
            .header(header::CONTENT_TYPE, "application/octet-stream")
            .header(header::CONTENT_LENGTH, length)
            .body(body)
            .send()
            .await?;
        Self::ok(Method::PUT, location.as_str(), response).await?;
        Ok(())
    }

    pub async fn push_bytes(
        &self,
        repository: &str,
        media_type: &str,
        body: &[u8],
    ) -> Result<Descriptor> {
        let descriptor = Descriptor::new(media_type, digest(body), body.len() as u64);
        if self.blob_exists(repository, &descriptor.digest).await? {
            return Ok(descriptor);
        }
        let location = self.start_upload(repository).await?;
        self.finish_upload(
            repository,
            location,
            &descriptor.digest,
            reqwest::Body::from(body.to_vec()),
            descriptor.size,
        )
        .await?;
        Ok(descriptor)
    }

    pub async fn push_stream<S, E>(
        &self,
        repository: &str,
        media_type: &str,
        digest: &str,
        size: u64,
        stream: S,
    ) -> Result<Descriptor>
    where
        S: Stream<Item = std::result::Result<Bytes, E>> + Send + 'static,
        E: Into<Box<dyn std::error::Error + Send + Sync>> + 'static,
    {
        let descriptor = Descriptor::new(media_type, digest.to_owned(), size);
        if self.blob_exists(repository, digest).await? {
            return Ok(descriptor);
        }
        let location = self.start_upload(repository).await?;
        self.finish_upload(
            repository,
            location,
            digest,
            reqwest::Body::wrap_stream(stream),
            size,
        )
        .await?;
        Ok(descriptor)
    }

    pub async fn push_file(
        &self,
        repository: &str,
        media_type: &str,
        path: &Path,
    ) -> Result<Descriptor> {
        let mut file = tokio::fs::File::open(path).await?;
        let mut hasher = Sha256::new();
        let mut buffer = vec![0u8; 1024 * 1024];
        loop {
            let read = file.read(&mut buffer).await?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
        }
        let size = file.metadata().await?.len();
        let descriptor = Descriptor::new(
            media_type,
            format!("sha256:{}", hex::encode(hasher.finalize())),
            size,
        );
        if self.blob_exists(repository, &descriptor.digest).await? {
            return Ok(descriptor);
        }

        let mut file = tokio::fs::File::open(path).await?;
        let mut location = self.start_upload(repository).await?;
        let mut offset = 0u64;
        let mut chunk = vec![0u8; self.chunk_size];
        while offset < size {
            let mut filled = 0;
            while filled < chunk.len() {
                let read = file.read(&mut chunk[filled..]).await?;
                if read == 0 {
                    break;
                }
                filled += read;
            }
            if filled == 0 {
                break;
            }

            let response = self
                .request(Method::PATCH, location.clone(), repository, true)
                .await?
                .header(header::CONTENT_TYPE, "application/octet-stream")
                .header(header::CONTENT_LENGTH, filled)
                .header(
                    "content-range",
                    format!("{offset}-{}", offset + filled as u64 - 1),
                )
                .body(chunk[..filled].to_vec())
                .send()
                .await?;
            let response = Self::ok(Method::PATCH, location.as_str(), response).await?;
            location = self.location(&response)?;
            offset += filled as u64;
        }

        self.finish_upload(
            repository,
            location,
            &descriptor.digest,
            reqwest::Body::from(Vec::new()),
            0,
        )
        .await?;
        Ok(descriptor)
    }

    pub async fn push_json<T: Serialize>(
        &self,
        repository: &str,
        reference: &str,
        media_type: &str,
        value: &T,
    ) -> Result<Descriptor> {
        let body = serde_json::to_vec(value)
            .map_err(|e| Error::Protocol(format!("encoding manifest: {e}")))?;
        let digest = self
            .put_manifest(repository, reference, media_type, &body)
            .await?;
        Ok(Descriptor::new(media_type, digest, body.len() as u64))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_config_digest_matches_the_spec() {
        assert_eq!(
            Descriptor::empty().digest,
            "sha256:44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a"
        );
    }

    #[test]
    fn challenges_parse() {
        let challenge =
            r#"Bearer realm="https://registry.anurag.sh/service/token",service="harbor-registry""#;
        assert_eq!(
            challenge_param(challenge, "realm"),
            Some("https://registry.anurag.sh/service/token")
        );
        assert_eq!(
            challenge_param(challenge, "service"),
            Some("harbor-registry")
        );
    }

    #[test]
    fn locations_are_rebased_onto_the_configured_registry() {
        let client = Client::new(
            reqwest::Client::new(),
            "http://harbor.registry.svc.cluster.local",
            None,
        )
        .unwrap();
        assert_eq!(
            client
                .rebase("https://registry.anurag.sh/v2/x/blobs/uploads/abc?_state=s")
                .unwrap()
                .as_str(),
            "http://harbor.registry.svc.cluster.local/v2/x/blobs/uploads/abc?_state=s"
        );
        assert_eq!(
            client.rebase("/v2/x/blobs/uploads/abc").unwrap().as_str(),
            "http://harbor.registry.svc.cluster.local/v2/x/blobs/uploads/abc"
        );
    }

    #[test]
    fn manifests_serialize_with_oci_field_names() {
        let manifest = Manifest::artifact("application/vnd.example", vec![]);
        let value = serde_json::to_value(&manifest).unwrap();
        assert_eq!(value["schemaVersion"], 2);
        assert_eq!(value["mediaType"], MANIFEST);
        assert_eq!(value["artifactType"], "application/vnd.example");
        assert_eq!(value["config"]["mediaType"], EMPTY);
        assert!(value.get("subject").is_none());
    }
}
