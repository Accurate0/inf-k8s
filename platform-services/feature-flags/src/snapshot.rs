//! Holds the live evaluation [`Snapshot`] in memory and keeps it current. A Postgres
//! `LISTEN flag_changes` task reloads on every admin mutation and fans the new config
//! version out to connected `StreamEvents` subscribers.

use crate::cache::CacheClient;
use crate::engine::Engine;
use crate::error::AppResult;
use crate::model::Snapshot;
use crate::store::Store;
use sqlx::postgres::PgListener;
use std::collections::BTreeSet;
use std::sync::{Arc, RwLock};
use tokio::sync::broadcast;

const CHANNEL: &str = "flag_changes";

/// A published config change: the new version plus the flag keys whose evaluation may
/// have changed, so subscribers can invalidate selectively rather than re-diffing.
#[derive(Clone, Debug)]
pub struct ConfigUpdate {
    pub version: i64,
    pub changed_flag_keys: Arc<Vec<String>>,
}

pub struct SnapshotManager {
    store: Store,
    cache: Option<CacheClient>,
    current: RwLock<Arc<Snapshot>>,
    tx: broadcast::Sender<ConfigUpdate>,
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

    pub fn subscribe(&self) -> broadcast::Receiver<ConfigUpdate> {
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

        let changed_flag_keys = {
            let mut current = self.current.write().unwrap();
            if version <= current.version {
                return Ok(());
            }
            let changed = changed_flag_keys(&current, &snapshot);
            *current = Arc::new(snapshot);
            changed
        };

        let _ = self.tx.send(ConfigUpdate {
            version,
            changed_flag_keys: Arc::new(changed_flag_keys),
        });
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

/// Flags whose resolved value may differ between two snapshots: those added, removed,
/// or directly modified, plus any flag whose rules reference a segment that changed.
fn changed_flag_keys(old: &Snapshot, new: &Snapshot) -> Vec<String> {
    let mut changed: BTreeSet<&str> = BTreeSet::new();

    for key in old.flags.keys().chain(new.flags.keys()) {
        if old.flags.get(key) != new.flags.get(key) {
            changed.insert(key);
        }
    }

    let changed_segments: BTreeSet<&str> = old
        .segments
        .keys()
        .chain(new.segments.keys())
        .filter(|key| old.segments.get(*key) != new.segments.get(*key))
        .map(String::as_str)
        .collect();
    if !changed_segments.is_empty() {
        for flag in new.flags.values() {
            let touched = flag.rules.iter().any(|r| {
                r.segment_key
                    .as_deref()
                    .is_some_and(|s| changed_segments.contains(s))
            });
            if touched {
                changed.insert(&flag.key);
            }
        }
    }

    changed.into_iter().map(String::from).collect()
}

#[cfg(test)]
mod tests {
    use super::changed_flag_keys;
    use crate::model::{Constraint, Flag, Operator, Rule, Segment, Snapshot, ValueType, Variant};
    use serde_json::json;

    fn flag(key: &str, enabled: bool, segment: Option<&str>) -> Flag {
        Flag {
            key: key.into(),
            value_type: ValueType::Boolean,
            enabled,
            default_variant_key: "off".into(),
            archived: false,
            variants: vec![Variant { key: "off".into(), value: json!(false) }],
            rules: segment
                .map(|s| Rule {
                    rank: 0,
                    segment_key: Some(s.into()),
                    variant_key: Some("off".into()),
                    distributions: vec![],
                    constraint_groups: vec![],
                    bucket_salt: String::new(),
                })
                .into_iter()
                .collect(),
        }
    }

    fn segment(key: &str, value: &str) -> Segment {
        Segment {
            key: key.into(),
            name: key.into(),
            constraints: vec![Constraint {
                attribute: "country".into(),
                operator: Operator::Eq,
                values: vec![json!(value)],
            }],
        }
    }

    fn snapshot(version: i64, flags: Vec<Flag>, segments: Vec<Segment>) -> Snapshot {
        Snapshot {
            version,
            flags: flags.into_iter().map(|f| (f.key.clone(), f)).collect(),
            segments: segments.into_iter().map(|s| (s.key.clone(), s)).collect(),
        }
    }

    #[test]
    fn reports_added_removed_and_modified_flags() {
        let old = snapshot(1, vec![flag("a", true, None), flag("b", true, None)], vec![]);
        let new = snapshot(
            2,
            vec![flag("a", false, None), flag("c", true, None)],
            vec![],
        );
        // a modified, b removed, c added.
        assert_eq!(changed_flag_keys(&old, &new), vec!["a", "b", "c"]);
    }

    #[test]
    fn unchanged_flag_is_not_reported() {
        let old = snapshot(1, vec![flag("a", true, None)], vec![]);
        let new = snapshot(2, vec![flag("a", true, None)], vec![]);
        assert!(changed_flag_keys(&old, &new).is_empty());
    }

    #[test]
    fn segment_change_propagates_to_referencing_flags() {
        let old = snapshot(1, vec![flag("a", true, Some("s"))], vec![segment("s", "AU")]);
        let new = snapshot(2, vec![flag("a", true, Some("s"))], vec![segment("s", "NZ")]);
        assert_eq!(changed_flag_keys(&old, &new), vec!["a"]);
    }
}
