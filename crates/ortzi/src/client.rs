use std::{fmt, path::Path, time::Duration};

use anyhow::Context;
use futures::TryStreamExt;
use registry_api::{ArtifactUrl, ErrorBody, SHA256_HEADER};
use reqwest::{Method, RequestBuilder, Response, StatusCode};
use serde::{Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::io::ReaderStream;

#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: String,
    pub message: String,
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({}): {}", self.code, self.status, self.message)
    }
}

impl std::error::Error for ApiError {}

pub fn api_error(e: &anyhow::Error) -> Option<&ApiError> {
    e.downcast_ref::<ApiError>()
}

const UPLOAD_TIMEOUT: Duration = Duration::from_secs(30 * 60);

pub struct Client {
    base: String,
    bearer: String,
    http: reqwest::Client,
    uploads: reqwest::Client,
}

fn http_client() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .user_agent(concat!("ortzi/", env!("ORTZI_VERSION")))
        .connect_timeout(Duration::from_secs(10))
}

impl Client {
    pub fn new(base: &str, bearer: String) -> Self {
        Self {
            base: base.trim_end_matches('/').to_owned(),
            bearer,
            http: http_client()
                .read_timeout(Duration::from_secs(60))
                .build()
                .expect("failed to build http client"),
            uploads: http_client()
                .timeout(UPLOAD_TIMEOUT)
                .build()
                .expect("failed to build http client"),
        }
    }

    fn request(&self, method: Method, path: &str) -> RequestBuilder {
        self.http
            .request(method, format!("{}{path}", self.base))
            .bearer_auth(&self.bearer)
    }

    async fn check(response: Response) -> anyhow::Result<Response> {
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }

        let body = response.text().await.unwrap_or_default();
        let (code, message) = match serde_json::from_str::<ErrorBody>(&body) {
            Ok(error) => (error.error.code, error.error.message),
            Err(_) => ("http".to_owned(), body),
        };
        Err(ApiError {
            status,
            code,
            message,
        }
        .into())
    }

    async fn json<T: DeserializeOwned>(request: RequestBuilder) -> anyhow::Result<T> {
        let response = Self::check(request.send().await?).await?;
        Ok(response.json().await?)
    }

    pub async fn get<T: DeserializeOwned>(&self, path: &str) -> anyhow::Result<T> {
        Self::json(self.request(Method::GET, path)).await
    }

    pub async fn find<T: DeserializeOwned>(&self, path: &str) -> anyhow::Result<Option<T>> {
        match self.get(path).await {
            Ok(value) => Ok(Some(value)),
            Err(e) if api_error(&e).is_some_and(|e| e.status == StatusCode::NOT_FOUND) => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub async fn send<B: Serialize, T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: &B,
    ) -> anyhow::Result<T> {
        Self::json(self.request(method, path).json(body)).await
    }

    pub async fn post_empty<T: DeserializeOwned>(&self, path: &str) -> anyhow::Result<T> {
        Self::json(self.request(Method::POST, path)).await
    }

    pub async fn delete(&self, path: &str) -> anyhow::Result<()> {
        Self::check(self.request(Method::DELETE, path).send().await?).await?;
        Ok(())
    }

    pub async fn download_artifact(&self, path: &str, dest: &Path) -> anyhow::Result<String> {
        match self.find::<ArtifactUrl>(&format!("{path}/url")).await? {
            Some(presigned) => {
                let response = self
                    .http
                    .get(&presigned.url)
                    .send()
                    .await
                    .context("downloading from object storage")?;
                write_body(Self::check(response).await?, dest).await
            }
            None => self.download(path, dest).await,
        }
    }

    pub async fn download(&self, path: &str, dest: &Path) -> anyhow::Result<String> {
        let response = Self::check(self.request(Method::GET, path).send().await?).await?;
        write_body(response, dest).await
    }

    pub async fn upload<T: DeserializeOwned>(
        &self,
        path: &str,
        file: &Path,
        sha256: &str,
    ) -> anyhow::Result<T> {
        let handle = tokio::fs::File::open(file)
            .await
            .with_context(|| format!("opening {}", file.display()))?;
        let size = handle.metadata().await?.len();
        let body = reqwest::Body::wrap_stream(ReaderStream::new(handle));

        Self::json(
            self.uploads
                .put(format!("{}{path}", self.base))
                .bearer_auth(&self.bearer)
                .header(SHA256_HEADER, sha256)
                .header(reqwest::header::CONTENT_LENGTH, size)
                .body(body),
        )
        .await
    }
}

async fn write_body(response: Response, dest: &Path) -> anyhow::Result<String> {
    let mut file = tokio::fs::File::create(dest)
        .await
        .with_context(|| format!("creating {}", dest.display()))?;
    let mut hasher = Sha256::new();
    let mut stream = response.bytes_stream();

    while let Some(chunk) = stream.try_next().await? {
        hasher.update(&chunk);
        file.write_all(&chunk).await?;
    }
    file.flush().await?;

    Ok(hex::encode(hasher.finalize()))
}

pub async fn sha256_file(path: &Path) -> anyhow::Result<String> {
    let mut file = tokio::fs::File::open(path)
        .await
        .with_context(|| format!("reading {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

pub fn human_size(bytes: i64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_are_human_readable() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(8_637_072), "8.2 MiB");
        assert_eq!(human_size(177_960_386), "169.7 MiB");
        assert_eq!(human_size(3 * 1024 * 1024 * 1024), "3.0 GiB");
    }
}
