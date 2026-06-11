//! Optional Dragonfly L2 cache for the evaluation snapshot. Lets a freshly started
//! replica warm up from cache instead of rebuilding the snapshot from Postgres, and
//! shares the compiled snapshot across replicas. Mirrors ai-gateway's `CacheClient`:
//! when `REDIS_URL` is unset the service runs with caching disabled.

use crate::model::Snapshot;
use redis::AsyncCommands;
use redis::aio::ConnectionManager;

const SNAPSHOT_KEY: &str = "ff:snapshot";

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

    pub async fn get_snapshot(&self) -> Option<Snapshot> {
        let mut conn = self.conn.clone();
        let raw: Option<Vec<u8>> = conn.get(SNAPSHOT_KEY).await.ok().flatten();
        raw.and_then(|bytes| serde_json::from_slice(&bytes).ok())
    }

    pub async fn put_snapshot(&self, snapshot: &Snapshot) {
        let Ok(bytes) = serde_json::to_vec(snapshot) else {
            return;
        };
        let mut conn = self.conn.clone();
        let _: Result<(), _> = conn.set(SNAPSHOT_KEY, bytes).await;
    }
}
