use redis::AsyncCommands;
use redis::aio::ConnectionManager;
use serde::Serialize;
use serde::de::DeserializeOwned;

/// Thin wrapper over a Dragonfly (redis-protocol) connection, shared by every replica so
/// cached keys, budgets and throttles stay consistent across the fleet. Optional: when
/// `REDIS_URL` is unset the gateway runs uncached and falls back to Postgres per request.
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

    pub async fn get_json<T: DeserializeOwned>(&self, key: &str) -> Option<T> {
        let mut conn = self.conn.clone();
        let raw: Option<Vec<u8>> = conn.get(key).await.ok().flatten();
        raw.and_then(|bytes| serde_json::from_slice(&bytes).ok())
    }

    pub async fn set_json<T: Serialize>(&self, key: &str, ttl_secs: u64, value: &T) {
        let Ok(bytes) = serde_json::to_vec(value) else {
            return;
        };
        let mut conn = self.conn.clone();
        let _: Result<(), _> = conn.set_ex(key, bytes, ttl_secs).await;
    }

    pub async fn get_i64(&self, key: &str) -> Option<i64> {
        let mut conn = self.conn.clone();
        conn.get(key).await.ok().flatten()
    }

    pub async fn set_i64(&self, key: &str, ttl_secs: u64, value: i64) {
        let mut conn = self.conn.clone();
        let _: Result<(), _> = conn.set_ex(key, value, ttl_secs).await;
    }

    /// `SET key NX EX ttl`. Returns true only if the key was absent and is now set, giving
    /// callers a fleet-wide "once per ttl" throttle. Treats redis errors as not-claimed.
    pub async fn claim_throttle(&self, key: &str, ttl_secs: u64) -> bool {
        let mut conn = self.conn.clone();
        let res: Option<String> = redis::cmd("SET")
            .arg(key)
            .arg(1)
            .arg("NX")
            .arg("EX")
            .arg(ttl_secs)
            .query_async(&mut conn)
            .await
            .ok()
            .flatten();
        res.as_deref() == Some("OK")
    }

    /// Deletes every key matching `pattern` via SCAN, used to flush cached keys after a
    /// mutation so the change takes effect immediately rather than waiting out the TTL.
    pub async fn invalidate(&self, pattern: &str) {
        let mut conn = self.conn.clone();
        let mut cursor: u64 = 0;
        loop {
            let res: Result<(u64, Vec<String>), _> = redis::cmd("SCAN")
                .arg(cursor)
                .arg("MATCH")
                .arg(pattern)
                .arg("COUNT")
                .arg(100)
                .query_async(&mut conn)
                .await;
            let Ok((next, keys)) = res else {
                return;
            };
            if !keys.is_empty() {
                let _: Result<(), _> = conn.del(keys).await;
            }
            if next == 0 {
                break;
            }
            cursor = next;
        }
    }
}
