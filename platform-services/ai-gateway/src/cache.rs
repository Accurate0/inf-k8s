use redis::AsyncCommands;
use redis::aio::ConnectionManager;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// A cached non-streaming upstream response, including the token usage parsed from it
/// so cache hits still produce accurate (zero-cost) usage rows.
#[derive(Clone, Serialize, Deserialize)]
pub struct CachedResponse {
    pub status: u16,
    pub body: Vec<u8>,
    pub input_tokens: i64,
    pub output_tokens: i64,
}

/// Thin wrapper over a Dragonfly (redis-protocol) connection. Optional: when
/// `REDIS_URL` is unset the gateway runs with caching disabled.
#[derive(Clone)]
pub struct CacheClient {
    conn: ConnectionManager,
}

impl CacheClient {
    pub async fn from_env() -> Option<Self> {
        let url = std::env::var("REDIS_URL").ok().filter(|s| !s.is_empty())?;
        match redis::Client::open(url) {
            Ok(client) => match client.get_connection_manager().await {
                Ok(conn) => Some(Self { conn }),
                Err(e) => {
                    tracing::error!("failed to connect to dragonfly, caching disabled: {e}");
                    None
                }
            },
            Err(e) => {
                tracing::error!("invalid REDIS_URL, caching disabled: {e}");
                None
            }
        }
    }

    pub async fn get(&self, key: &str) -> Option<CachedResponse> {
        let mut conn = self.conn.clone();
        let raw: Option<Vec<u8>> = conn.get(key).await.ok().flatten();
        raw.and_then(|bytes| serde_json::from_slice(&bytes).ok())
    }

    pub async fn put(&self, key: &str, ttl_secs: u64, value: &CachedResponse) {
        let Ok(bytes) = serde_json::to_vec(value) else {
            return;
        };
        let mut conn = self.conn.clone();
        let _: Result<(), _> = conn.set_ex(key, bytes, ttl_secs).await;
    }
}

/// Deterministic cache key over the route, client endpoint, and request body. The
/// endpoint matters because the cached body is in the client's dialect.
pub fn cache_key(provider: &str, model: &str, endpoint: &str, body: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(provider.as_bytes());
    hasher.update(b"|");
    hasher.update(model.as_bytes());
    hasher.update(b"|");
    hasher.update(endpoint.as_bytes());
    hasher.update(b"|");
    hasher.update(body);
    format!("aig:cache:{}", hex::encode(hasher.finalize()))
}
