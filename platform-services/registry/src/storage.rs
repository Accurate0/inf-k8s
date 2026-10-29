use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use anyhow::Context;
use bytes::Bytes;
use futures::{Stream, StreamExt, TryStreamExt, stream::BoxStream};
use s3::{Bucket, Region, creds::Credentials};
use sha2::{Digest, Sha256};
use tokio_util::io::StreamReader;

use crate::{config::StorageConfig, error::ApiError};

const TRANSFER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3600);

#[derive(Clone)]
pub enum Storage {
    S3(Box<Bucket>),
    Memory(Arc<Mutex<HashMap<String, Bytes>>>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stored {
    pub sha256: String,
    pub size: u64,
}

#[derive(Default)]
struct Progress {
    hasher: Sha256,
    size: u64,
    exceeded: bool,
}

impl Storage {
    pub fn from_env(config: &StorageConfig) -> anyhow::Result<Self> {
        let access_key =
            std::env::var("S3_ACCESS_KEY_ID").context("S3_ACCESS_KEY_ID must be set")?;
        let secret_key =
            std::env::var("S3_SECRET_ACCESS_KEY").context("S3_SECRET_ACCESS_KEY must be set")?;
        let endpoint = std::env::var("S3_ENDPOINT").context("S3_ENDPOINT must be set")?;

        let credentials = Credentials::new(Some(&access_key), Some(&secret_key), None, None, None)
            .context("building S3 credentials")?;
        let region = Region::Custom {
            region: config.region.clone(),
            endpoint,
        };

        let mut bucket = Bucket::new(&config.bucket, region, credentials)?.with_path_style();
        bucket.set_request_timeout(Some(TRANSFER_TIMEOUT));
        Ok(Storage::S3(bucket))
    }

    pub fn memory() -> Self {
        Storage::Memory(Arc::default())
    }

    pub async fn put_stream<S, E>(
        &self,
        key: &str,
        body: S,
        max_bytes: u64,
    ) -> Result<Stored, ApiError>
    where
        S: Stream<Item = Result<Bytes, E>> + Send + Unpin,
        E: std::fmt::Display,
    {
        let progress = Arc::new(Mutex::new(Progress::default()));
        let tracked = progress.clone();

        let body = body.map(move |chunk| {
            let chunk = chunk.map_err(|e| std::io::Error::other(e.to_string()))?;
            let mut progress = tracked.lock().unwrap();
            progress.size += chunk.len() as u64;
            if progress.size > max_bytes {
                progress.exceeded = true;
                return Err(std::io::Error::other("artifact too large"));
            }
            progress.hasher.update(&chunk);
            Ok(chunk)
        });

        let result = match self {
            Storage::S3(bucket) => {
                let mut reader = StreamReader::new(body);
                bucket
                    .put_object_stream_with_content_type(
                        &mut reader,
                        key,
                        "application/octet-stream",
                    )
                    .await
                    .map_err(|e| e.to_string())
                    .and_then(|response| match response.status_code() {
                        200..=299 => Ok(()),
                        status => Err(format!("unexpected status {status} uploading {key}")),
                    })
            }
            Storage::Memory(objects) => match body.try_collect::<Vec<Bytes>>().await {
                Ok(chunks) => {
                    objects
                        .lock()
                        .unwrap()
                        .insert(key.to_owned(), chunks.concat().into());
                    Ok(())
                }
                Err(e) => Err(e.to_string()),
            },
        };

        let progress = std::mem::take(&mut *progress.lock().unwrap());
        if progress.exceeded {
            return Err(ApiError::TooLarge(max_bytes));
        }
        result.map_err(ApiError::Storage)?;

        Ok(Stored {
            sha256: hex::encode(progress.hasher.finalize()),
            size: progress.size,
        })
    }

    pub async fn get_stream(
        &self,
        key: &str,
    ) -> Result<BoxStream<'static, std::io::Result<Bytes>>, ApiError> {
        match self {
            Storage::S3(bucket) => {
                let response = bucket
                    .get_object_stream(key)
                    .await
                    .map_err(|e| ApiError::Storage(e.to_string()))?;
                if response.status_code != 200 {
                    return Err(ApiError::Storage(format!(
                        "unexpected status {} fetching {key}",
                        response.status_code
                    )));
                }
                Ok(response
                    .bytes
                    .map_err(|e| std::io::Error::other(e.to_string()))
                    .boxed())
            }
            Storage::Memory(objects) => {
                let object = objects
                    .lock()
                    .unwrap()
                    .get(key)
                    .cloned()
                    .ok_or_else(|| ApiError::Storage(format!("missing object {key}")))?;
                Ok(futures::stream::once(async move { Ok(object) }).boxed())
            }
        }
    }

    pub async fn delete(&self, key: &str) -> Result<(), ApiError> {
        match self {
            Storage::S3(bucket) => {
                let response = bucket
                    .delete_object(key)
                    .await
                    .map_err(|e| ApiError::Storage(e.to_string()))?;
                match response.status_code() {
                    200..=299 | 404 => Ok(()),
                    status => Err(ApiError::Storage(format!(
                        "unexpected status {status} deleting {key}"
                    ))),
                }
            }
            Storage::Memory(objects) => {
                objects.lock().unwrap().remove(key);
                Ok(())
            }
        }
    }

    #[cfg(test)]
    pub fn contains(&self, key: &str) -> bool {
        match self {
            Storage::Memory(objects) => objects.lock().unwrap().contains_key(key),
            Storage::S3(_) => unimplemented!(),
        }
    }
}

pub fn cli_artifact_key(package: &str, version: &str, target: &str, binary: &str) -> String {
    format!("cli/packages/{package}/{version}/{target}/{binary}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(parts: &[&'static [u8]]) -> impl Stream<Item = Result<Bytes, std::io::Error>> + Unpin {
        futures::stream::iter(
            parts
                .iter()
                .map(|p| Ok(Bytes::from_static(p)))
                .collect::<Vec<_>>(),
        )
    }

    #[tokio::test]
    async fn hashes_while_streaming() {
        let storage = Storage::memory();
        let stored = storage
            .put_stream("k", body(&[b"hello ", b"world"]), 1024)
            .await
            .unwrap();

        assert_eq!(stored.size, 11);
        assert_eq!(stored.sha256, hex::encode(Sha256::digest(b"hello world")));

        let read: Vec<Bytes> = storage
            .get_stream("k")
            .await
            .unwrap()
            .try_collect()
            .await
            .unwrap();
        assert_eq!(read.concat(), b"hello world");
    }

    #[tokio::test]
    async fn rejects_oversized_bodies() {
        let storage = Storage::memory();
        let err = storage
            .put_stream("k", body(&[b"0123456789"]), 4)
            .await
            .unwrap_err();

        assert!(matches!(err, ApiError::TooLarge(4)));
        assert!(!storage.contains("k"));
    }
}
