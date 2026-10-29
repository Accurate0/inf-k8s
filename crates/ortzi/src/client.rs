use std::{fmt, path::Path};

use anyhow::Context;
use futures::TryStreamExt;
use registry_api::{ErrorBody, SHA256_HEADER};
use reqwest::{Method, RequestBuilder, Response, StatusCode};
use serde::{Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
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

pub struct Client {
    base: String,
    bearer: String,
    http: reqwest::Client,
}

impl Client {
    pub fn new(base: &str, bearer: String) -> Self {
        Self {
            base: base.trim_end_matches('/').to_owned(),
            bearer,
            http: reqwest::Client::builder()
                .user_agent(concat!("ortzi/", env!("ORTZI_VERSION")))
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

    pub async fn download(&self, path: &str, dest: &Path) -> anyhow::Result<String> {
        let response = Self::check(self.request(Method::GET, path).send().await?).await?;
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
            self.request(Method::PUT, path)
                .header(SHA256_HEADER, sha256)
                .header(reqwest::header::CONTENT_LENGTH, size)
                .body(body),
        )
        .await
    }
}

pub async fn sha256_file(path: &Path) -> anyhow::Result<String> {
    let bytes = tokio::fs::read(path)
        .await
        .with_context(|| format!("reading {}", path.display()))?;
    Ok(hex::encode(Sha256::digest(&bytes)))
}
