use axum::{
    body::Body,
    http::{HeaderValue, StatusCode},
    response::Response,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::cache::CacheClient;
use crate::providers::Dialect;

const NAMESPACE: &str = "aig:resp:";
const DEFAULT_TTL_SECS: u64 = 3600;

/// A buffered upstream response, stored verbatim in the client's dialect so a hit can be
/// replayed without contacting a provider.
#[derive(Serialize, Deserialize)]
pub struct CachedResponse {
    pub status: u16,
    pub content_type: String,
    pub body: Vec<u8>,
    pub input_tokens: i64,
    pub output_tokens: i64,
}

/// Cache key for a request: its exact body under the client dialect and sub-path, so two
/// dialects or endpoints never collide. Independent of the virtual key, since identical
/// requests yield identical answers and access control is enforced before lookup.
pub fn key(dialect: Dialect, sub_path: &str, body: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update([dialect as u8]);
    hasher.update(sub_path.as_bytes());
    hasher.update(body);
    format!("{NAMESPACE}{}", hex::encode(hasher.finalize()))
}

fn ttl_secs() -> u64 {
    std::env::var("RESPONSE_CACHE_TTL_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_TTL_SECS)
}

pub async fn get(cache: &CacheClient, key: &str) -> Option<CachedResponse> {
    cache.get_json(key).await
}

pub async fn put(cache: &CacheClient, key: &str, value: &CachedResponse) {
    cache.set_json(key, ttl_secs(), value).await;
}

impl CachedResponse {
    pub fn into_response(self) -> Response {
        let content_type = HeaderValue::from_str(&self.content_type)
            .unwrap_or_else(|_| HeaderValue::from_static("application/json"));
        Response::builder()
            .status(StatusCode::from_u16(self.status).unwrap_or(StatusCode::OK))
            .header("content-type", content_type)
            .header("x-cache", "HIT")
            .body(Body::from(self.body))
            .unwrap()
    }
}
