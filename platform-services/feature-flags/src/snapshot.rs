//! Holds the live evaluation [`Snapshot`] in memory and keeps it current. A Postgres
//! `LISTEN flag_changes` task reloads on every admin mutation and fans the new config
//! version out to connected `StreamEvents` subscribers.

use crate::cache::CacheClient;
use crate::engine::Engine;
use crate::error::AppResult;
use crate::model::Snapshot;
use crate::store::Store;
use sqlx::postgres::PgListener;
use std::sync::{Arc, RwLock};
use tokio::sync::broadcast;

const CHANNEL: &str = "flag_changes";

pub struct SnapshotManager {
    store: Store,
    cache: Option<CacheClient>,
    current: RwLock<Arc<Snapshot>>,
    tx: broadcast::Sender<i64>,
}

impl SnapshotManager {
    pub async fn bootstrap(store: Store, cache: Option<CacheClient>) -> AppResult<Arc<Self>> {
        let snapshot = Self::initial(&store, cache.as_ref()).await?;
        let (tx, _) = broadcast::channel(64);
        Ok(Arc::new(Self {
            store,
            cache,
            current: RwLock::new(Arc::new(snapshot)),
            tx,
        }))
    }

    async fn initial(store: &Store, cache: Option<&CacheClient>) -> AppResult<Snapshot> {
        let version = store.config_version().await?;
        if let Some(cache) = cache
            && let Some(cached) = cache.get_snapshot().await
            && cached.version == version
        {
            return Ok(cached);
        }
        let snapshot = store.load_snapshot().await?;
        if let Some(cache) = cache {
            cache.put_snapshot(&snapshot).await;
        }
        Ok(snapshot)
    }

    pub fn engine(&self) -> Engine {
        Engine::new(self.current.read().unwrap().clone())
    }

    pub fn version(&self) -> i64 {
        self.current.read().unwrap().version
    }

    pub fn subscribe(&self) -> broadcast::Receiver<i64> {
        self.tx.subscribe()
    }

    /// Reload the snapshot from Postgres and, if it advanced the config version,
    /// publish it. A single admin write triggers both an explicit reload (for
    /// read-your-writes) and the LISTEN/NOTIFY reload; the version guard collapses
    /// those into one swap and one broadcast.
    pub async fn reload(&self) -> AppResult<()> {
        let snapshot = self.store.load_snapshot().await?;
        let version = snapshot.version;
        if version <= self.version() {
            return Ok(());
        }
        if let Some(cache) = &self.cache {
            cache.put_snapshot(&snapshot).await;
        }
        {
            let mut current = self.current.write().unwrap();
            if version <= current.version {
                return Ok(());
            }
            *current = Arc::new(snapshot);
        }
        let _ = self.tx.send(version);
        Ok(())
    }

    /// Long-lived task: reload whenever Postgres notifies a config change. On listener
    /// errors it reconnects with a short backoff so a transient DB blip is self-healing.
    pub async fn listen(self: Arc<Self>, database_url: String) {
        loop {
            match PgListener::connect(&database_url).await {
                Ok(mut listener) => {
                    if let Err(e) = listener.listen(CHANNEL).await {
                        tracing::error!("failed to LISTEN {CHANNEL}: {e}");
                        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                        continue;
                    }
                    tracing::info!("listening for flag changes on `{CHANNEL}`");
                    while let Ok(notification) = listener.recv().await {
                        tracing::debug!(payload = notification.payload(), "flag change notified");
                        if let Err(e) = self.reload().await {
                            tracing::error!("snapshot reload failed: {e}");
                        }
                    }
                    tracing::warn!("flag change listener disconnected, reconnecting");
                }
                Err(e) => {
                    tracing::error!("failed to connect flag change listener: {e}");
                }
            }
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }
    }
}
