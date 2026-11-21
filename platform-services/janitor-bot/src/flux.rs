use serde::Deserialize;
use std::collections::HashMap;

use crate::event::SyncEvent;
use crate::forgejo::ForgejoClient;

pub const CLUSTER_ARTIFACT: &str = "oci://registry.anurag.sh/manifests/cluster";

const LOCATIONS: [&str; 3] = ["platform-services", "projects", "selfhosted"];

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ObjectRef {
    pub kind: String,
    pub name: String,
}

/// A notification-controller event; `metadata` carries the object's
/// `event.toolkit.fluxcd.io/*` annotations with the prefix stripped.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FluxEvent {
    pub involved_object: ObjectRef,
    pub severity: String,
    pub reason: String,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub metadata: HashMap<String, String>,
}

impl FluxEvent {
    /// The app this event reports on, when it is about an app's own kustomization.
    pub fn app(&self) -> Option<&str> {
        let app = self.metadata.get("app")?;
        (self.involved_object.kind == "Kustomization" && self.involved_object.name == *app)
            .then_some(app.as_str())
    }

    /// The commit the app was published from, or the cluster artifact digest to resolve one from.
    pub fn commit(&self) -> Option<&str> {
        self.metadata.get("commit").map(String::as_str).or_else(|| {
            self.metadata
                .get("revision")
                .and_then(|revision| revision.split_once('@'))
                .map(|(_, digest)| digest)
        })
    }

    pub fn to_sync(&self, app: &str, sha: String) -> SyncEvent {
        let (phase, health) = if self.severity == "error" {
            ("Failed", "Degraded")
        } else if self.reason.ends_with("Succeeded") {
            ("Succeeded", "Healthy")
        } else {
            ("Running", "Progressing")
        };
        SyncEvent {
            app_name: app.to_owned(),
            sha,
            sync_status: "Synced".to_owned(),
            health_status: health.to_owned(),
            phase: phase.to_owned(),
            message: self.message.clone(),
        }
    }
}

fn touches(file: &str, app: &str) -> bool {
    LOCATIONS
        .iter()
        .any(|location| file.starts_with(&format!("{location}/{app}/")))
        || file
            .strip_prefix("kubernetes/")
            .is_some_and(|rest| rest.split('/').any(|segment| segment == app))
}

pub async fn app_changed_in_commit(
    forgejo: &ForgejoClient,
    owner: &str,
    repo: &str,
    sync: &SyncEvent,
) -> bool {
    match forgejo
        .get_commit_changed_files(owner, repo, &sync.sha)
        .await
    {
        Ok(files) => files.iter().any(|file| touches(file, &sync.app_name)),
        Err(e) => {
            tracing::warn!(
                app = sync.app_name,
                sha = sync.sha,
                "failed to get commit changed files: {e}"
            );
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_app_is_touched_by_its_source_or_its_kubernetes_dir() {
        assert!(touches("platform-services/graph/app.yaml", "graph"));
        assert!(touches("kubernetes/graph/app/helmrelease.yaml", "graph"));
        assert!(touches("kubernetes/kube-system/etcd-backup/ks.yaml", "etcd-backup"));
        assert!(!touches("kubernetes/graph-db/ks.yaml", "graph"));
        assert!(!touches("platform-services/graphite/app.yaml", "graph"));
    }

    #[test]
    fn only_an_apps_own_kustomization_reports_on_it() {
        let event: FluxEvent = serde_json::from_value(serde_json::json!({
            "involvedObject": {"kind": "HelmRelease", "name": "graph-k8s-workload"},
            "severity": "info",
            "reason": "UpgradeSucceeded",
            "metadata": {"app": "graph"}
        }))
        .unwrap();
        assert_eq!(event.app(), None);
    }

    #[test]
    fn the_stamped_commit_wins_over_the_artifact_digest() {
        let event: FluxEvent = serde_json::from_value(serde_json::json!({
            "involvedObject": {"kind": "Kustomization", "name": "graph"},
            "severity": "info",
            "reason": "ReconciliationSucceeded",
            "metadata": {"app": "graph", "commit": "abc", "revision": "main@sha256:def"}
        }))
        .unwrap();
        assert_eq!(event.app(), Some("graph"));
        assert_eq!(event.commit(), Some("abc"));
    }
}
