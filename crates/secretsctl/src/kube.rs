use std::process::Command;

use anyhow::{Context, bail};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct ExternalSecret {
    pub metadata: Meta,
    pub spec: Spec,
    #[serde(default)]
    pub status: Status,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Meta {
    pub name: String,
    pub namespace: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Spec {
    #[serde(default)]
    pub secret_store_ref: StoreRef,
    #[serde(default)]
    pub data_from: Vec<DataFrom>,
    #[serde(default)]
    pub data: Vec<Data>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct StoreRef {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub kind: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DataFrom {
    #[serde(default)]
    pub extract: Option<Key>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Data {
    pub remote_ref: Key,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Key {
    pub key: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    #[serde(default)]
    pub refresh_time: Option<String>,
    #[serde(default)]
    pub conditions: Vec<Condition>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Condition {
    #[serde(rename = "type")]
    pub kind: String,
    pub status: String,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
}

impl ExternalSecret {
    pub fn reads_openbao(&self) -> bool {
        self.spec.secret_store_ref.kind != "ClusterSecretStore"
            && self.spec.secret_store_ref.name.starts_with("openbao")
    }

    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.spec
            .data_from
            .iter()
            .filter_map(|d| d.extract.as_ref().map(|k| k.key.as_str()))
            .chain(self.spec.data.iter().map(|d| d.remote_ref.key.as_str()))
    }

    pub fn ready(&self) -> Option<&Condition> {
        self.status.conditions.iter().find(|c| c.kind == "Ready")
    }

    pub fn is_ready(&self) -> bool {
        self.ready().is_some_and(|c| c.status == "True")
    }
}

pub fn matching<'a>(all: &'a [ExternalSecret], target: &str) -> Vec<&'a ExternalSecret> {
    let target = target.trim_matches('/');
    all.iter()
        .filter(|es| es.reads_openbao())
        .filter(|es| {
            if target.contains('/') {
                es.keys().any(|k| k.trim_matches('/') == target)
            } else {
                es.metadata.namespace == target || es.keys().any(|k| k.trim_matches('/') == target)
            }
        })
        .collect()
}

fn kubectl(args: &[&str]) -> anyhow::Result<String> {
    let output = Command::new("kubectl")
        .args(args)
        .output()
        .context("running kubectl")?;
    if !output.status.success() {
        bail!(
            "kubectl {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8(output.stdout)?)
}

pub fn list(namespace: Option<&str>) -> anyhow::Result<Vec<ExternalSecret>> {
    #[derive(Deserialize)]
    struct List {
        items: Vec<ExternalSecret>,
    }

    let mut args = vec!["get", "externalsecrets.external-secrets.io", "-o", "json"];
    match namespace {
        Some(ns) => args.extend(["-n", ns]),
        None => args.push("-A"),
    }
    Ok(serde_json::from_str::<List>(&kubectl(&args)?)?.items)
}

pub fn get(es: &ExternalSecret) -> anyhow::Result<ExternalSecret> {
    Ok(serde_json::from_str(&kubectl(&[
        "get",
        "externalsecrets.external-secrets.io",
        "-n",
        &es.metadata.namespace,
        &es.metadata.name,
        "-o",
        "json",
    ])?)?)
}

pub fn force_sync(es: &ExternalSecret, stamp: &str) -> anyhow::Result<()> {
    kubectl(&[
        "annotate",
        "externalsecrets.external-secrets.io",
        "-n",
        &es.metadata.namespace,
        &es.metadata.name,
        &format!("force-sync={stamp}"),
        "--overwrite",
    ])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"{"items":[
      {"metadata":{"name":"app-managed","namespace":"app"},
       "spec":{"secretStoreRef":{"name":"openbao-app","kind":"SecretStore"},
               "dataFrom":[{"extract":{"key":"app"}}]},
       "status":{"conditions":[{"type":"Ready","status":"True","reason":"SecretSynced"}]}},
      {"metadata":{"name":"db-managed","namespace":"app"},
       "spec":{"secretStoreRef":{"name":"openbao-app-db","kind":"SecretStore"},
               "dataFrom":[{"extract":{"key":"app/app-db"}}]}},
      {"metadata":{"name":"azure","namespace":"app"},
       "spec":{"secretStoreRef":{"name":"k8s-shared-vault","kind":"ClusterSecretStore"},
               "dataFrom":[{"extract":{"key":"app"}}]}},
      {"metadata":{"name":"other","namespace":"other"},
       "spec":{"secretStoreRef":{"name":"openbao-other","kind":"SecretStore"},
               "data":[{"secretKey":"X","remoteRef":{"key":"app/app-db"}}]}}
    ]}"#;

    fn fixture() -> Vec<ExternalSecret> {
        #[derive(Deserialize)]
        struct List {
            items: Vec<ExternalSecret>,
        }
        serde_json::from_str::<List>(FIXTURE).unwrap().items
    }

    fn names(found: Vec<&ExternalSecret>) -> Vec<&str> {
        found.iter().map(|es| es.metadata.name.as_str()).collect()
    }

    #[test]
    fn matches_exact_paths_across_namespaces() {
        let all = fixture();
        assert_eq!(names(matching(&all, "app/app-db")), ["db-managed", "other"]);
    }

    #[test]
    fn a_namespace_matches_its_openbao_secrets_only() {
        let all = fixture();
        assert_eq!(names(matching(&all, "app")), ["app-managed", "db-managed"]);
    }

    #[test]
    fn readiness_comes_from_the_ready_condition() {
        let all = fixture();
        assert!(all[0].is_ready());
        assert!(!all[1].is_ready());
    }
}
