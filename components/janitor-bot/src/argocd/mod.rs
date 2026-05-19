pub mod types;

use crate::event::PrEvent;
use crate::forgejo::ForgejoClient;
use crate::marker::Marker;
use std::sync::LazyLock;
use std::{collections::BTreeSet, process::Stdio};
use tokio::process::Command;
use types::{Application, ApplicationList, SourceDiff};

static COMMENT_MARKER: LazyLock<Marker> = LazyLock::new(|| Marker::feature("argocd-diff"));
const MAX_DIFF_LEN: usize = 60_000;
/// Identifier of the manifest repo; ArgoCD source paths are only meaningful
/// when the source points at this repo.
const MANIFEST_REPO: &str = "Accurate0/inf-k8s";

pub struct ArgocdClient {
    server: String,
    token: String,
    http: reqwest::Client,
}

impl ArgocdClient {
    pub fn new(server: String, token: String) -> Self {
        let http = reqwest::Client::builder()
            .build()
            .expect("failed to build argocd http client");

        Self {
            server,
            token,
            http,
        }
    }

    pub fn from_env() -> anyhow::Result<Self> {
        let server = std::env::var("ARGOCD_SERVER")?;
        let token = std::env::var("ARGOCD_AUTH_TOKEN")?;
        Ok(Self::new(server, token))
    }

    async fn diff(&self, app_name: &str, revision: &str, source_position: usize) -> String {
        let plaintext = self.server.starts_with("http://");
        let server = self
            .server
            .strip_prefix("https://")
            .or_else(|| self.server.strip_prefix("http://"))
            .unwrap_or(&self.server);

        let mut args = vec![
            "app".to_string(),
            "diff".to_string(),
            app_name.to_string(),
            "--server".to_string(),
            server.to_string(),
            "--auth-token".to_string(),
            self.token.clone(),
            "--insecure".to_string(),
            "--grpc-web".to_string(),
            "--revisions".to_string(),
            revision.to_string(),
            "--source-positions".to_string(),
            source_position.to_string(),
        ];

        if plaintext {
            args.push("--plaintext".to_string());
        }

        let result = Command::new("argocd")
            .args(&args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await;

        match result {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();

                if !stdout.is_empty() {
                    if stdout.len() > MAX_DIFF_LEN {
                        format!(
                            "{}...\n\n(truncated, diff too large)",
                            &stdout[..MAX_DIFF_LEN]
                        )
                    } else {
                        stdout
                    }
                } else if output.status.code() == Some(0) {
                    "No differences found.".to_string()
                } else {
                    let msg = serde_json::from_str::<serde_json::Value>(&stderr)
                        .ok()
                        .and_then(|v| v["msg"].as_str().map(String::from))
                        .unwrap_or(stderr);
                    format!(
                        "Error running argocd diff (exit {}): {}",
                        output.status, msg
                    )
                }
            }
            Err(e) => format!("Failed to run argocd CLI: {e}"),
        }
    }

    fn find_changed_sources(
        &self,
        old_content: &str,
        new_content: &str,
    ) -> anyhow::Result<Vec<SourceDiff>> {
        let new_app: Application = yaml_serde::from_str(new_content)?;
        let old_app: Application = yaml_serde::from_str(old_content)?;

        tracing::info!(
            app = new_app.metadata.name,
            new_sources = new_app.spec.sources.len(),
            old_sources = old_app.spec.sources.len(),
            "comparing sources"
        );

        let mut diffs = Vec::new();
        for (i, (new_src, old_src)) in new_app
            .spec
            .sources
            .iter()
            .zip(old_app.spec.sources.iter())
            .enumerate()
        {
            let new_rev = new_src.target_revision.as_deref().unwrap_or("");
            let old_rev = old_src.target_revision.as_deref().unwrap_or("");

            tracing::info!(
                source = i + 1,
                chart = new_src.chart.as_deref().unwrap_or("?"),
                old_rev,
                new_rev,
                "comparing source"
            );

            if new_rev != old_rev {
                diffs.push(SourceDiff {
                    app_name: new_app.metadata.name.clone(),
                    chart_name: new_src
                        .chart
                        .clone()
                        .unwrap_or_else(|| "unknown".to_string()),
                    old_revision: old_rev.to_string(),
                    new_revision: new_rev.to_string(),
                    source_position: i + 1,
                });
            }
        }
        Ok(diffs)
    }

    fn format_comment(&self, source_diffs: &[SourceDiff], diff_outputs: &[String]) -> String {
        let mut s = format!("{}\n## ArgoCD Diff\n", *COMMENT_MARKER);

        for (sd, diff_output) in source_diffs.iter().zip(diff_outputs.iter()) {
            s.push_str(&format!(
                "\n### `{}` — `{}` (`{}` → `{}`)\n\
                 <details>\n\
                 <summary>Diff (source {})</summary>\n\n\
                 ```diff\n\
                 {}\n\
                 ```\n\
                 </details>\n",
                sd.app_name,
                sd.chart_name,
                sd.old_revision,
                sd.new_revision,
                sd.source_position,
                diff_output,
            ));
        }

        s
    }

    fn is_application_yaml(path: &str) -> bool {
        path.ends_with(".application.yaml")
            || (path.ends_with("/application.yaml") && !path.contains("/manifests/"))
    }

    pub async fn run_diff_and_comment(
        &self,
        client: &ForgejoClient,
        pr: &PrEvent,
    ) -> anyhow::Result<()> {
        let app_files: Vec<&String> = pr
            .changed_files
            .iter()
            .filter(|f| Self::is_application_yaml(f))
            .collect();

        if app_files.is_empty() {
            tracing::warn!("no application.yaml files changed, skipping argocd diff");
            return Ok(());
        }

        if !client
            .is_pr_open(&pr.owner, &pr.repo, pr.pr_number as i64)
            .await
        {
            tracing::info!(pr = pr.pr_number, "PR is not open, skipping argocd diff");
            return Ok(());
        }

        let head_ref = client
            .get_pr_head_ref(&pr.owner, &pr.repo, pr.pr_number as i64)
            .await?;

        let mut all_source_diffs = Vec::new();

        for file in &app_files {
            let new_content = match client
                .get_raw_file(&pr.owner, &pr.repo, file, &head_ref)
                .await
            {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(file, "failed to fetch new file content: {e}");
                    continue;
                }
            };

            let old_content = match client
                .get_raw_file(&pr.owner, &pr.repo, file, &pr.target_branch)
                .await
            {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(file, "failed to fetch old file content: {e}");
                    continue;
                }
            };

            match self.find_changed_sources(&old_content, &new_content) {
                Ok(diffs) => {
                    tracing::info!(file, count = diffs.len(), "found source diffs");
                    all_source_diffs.extend(diffs);
                }
                Err(e) => {
                    tracing::warn!(file, "failed to parse application yaml: {e}");
                    continue;
                }
            }
        }

        if all_source_diffs.is_empty() {
            tracing::warn!("no targetRevision changes found, skipping argocd diff");
            return Ok(());
        }

        let mut diff_outputs = Vec::with_capacity(all_source_diffs.len());
        for sd in &all_source_diffs {
            tracing::info!(
                app = sd.app_name,
                chart = sd.chart_name,
                old = sd.old_revision,
                new = sd.new_revision,
                source_position = sd.source_position,
                "running argocd diff"
            );
            let output = self
                .diff(&sd.app_name, &sd.new_revision, sd.source_position)
                .await;
            diff_outputs.push(output);
        }

        let comment = self.format_comment(&all_source_diffs, &diff_outputs);
        client
            .comment_or_update(
                &pr.owner,
                &pr.repo,
                pr.pr_number as i64,
                &COMMENT_MARKER,
                &comment,
            )
            .await?;

        Ok(())
    }

    pub async fn check_app_changed_in_commit(
        &self,
        forgejo: &ForgejoClient,
        owner: &str,
        repo: &str,
        sync: &crate::event::ArgoSyncEvent,
    ) -> bool {
        let files = match forgejo
            .get_commit_changed_files(owner, repo, &sync.sha)
            .await
        {
            Ok(files) => files,
            Err(e) => {
                tracing::warn!(
                    app = sync.app_name,
                    sha = sync.sha,
                    "failed to get commit changed files: {e}"
                );
                return false;
            }
        };

        let candidates = [
            format!("applications/{}.application.yaml", sync.app_name),
            format!("projects/{}/application.yaml", sync.app_name),
        ];

        // Short-circuit: app file itself was changed
        if candidates.iter().any(|c| files.iter().any(|f| f == c)) {
            return true;
        }

        // Try fetching app yaml to check source paths
        let app_content = {
            let mut content = None;
            for candidate in &candidates {
                match forgejo
                    .get_raw_file(owner, repo, candidate, &sync.sha)
                    .await
                {
                    Ok(c) => {
                        content = Some(c);
                        break;
                    }
                    Err(_) => continue,
                }
            }
            content
        };

        let Some(app_content) = app_content else {
            tracing::warn!(
                app = sync.app_name,
                "could not fetch app yaml from any candidate path"
            );
            return false;
        };

        let app: types::Application = match yaml_serde::from_str(&app_content) {
            Ok(app) => app,
            Err(e) => {
                tracing::warn!(app = sync.app_name, "failed to parse app yaml: {e}");
                return false;
            }
        };

        app.spec.sources.iter().any(|source| {
            if let (Some(path), None) = (&source.path, &source.chart) {
                let prefix = if path.ends_with('/') {
                    path.clone()
                } else {
                    format!("{path}/")
                };
                files.iter().any(|f| f.starts_with(&prefix))
            } else {
                false
            }
        })
    }

    fn app_affected(app_name: &str, source_paths: &[String], changed_files: &[String]) -> bool {
        // The app's own definition file changed (chart bumps, source edits).
        let def_files = [
            format!("applications/{app_name}.application.yaml"),
            format!("projects/{app_name}/application.yaml"),
        ];

        if changed_files
            .iter()
            .any(|f| def_files.iter().any(|d| d == f))
        {
            return true;
        }

        source_paths.iter().any(|p| {
            let p = p.trim_matches('/');
            !p.is_empty()
                && changed_files
                    .iter()
                    .any(|f| f == p || f.starts_with(&format!("{p}/")))
        })
    }

    /// Fetches every application registered with ArgoCD.
    async fn list_applications(&self) -> anyhow::Result<Vec<Application>> {
        let url = format!("{}/api/v1/applications", self.api_base());
        let resp = self
            .http
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.token))
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("argocd list applications failed: {status} — {body}");
        }

        Ok(resp.json::<ApplicationList>().await?.items)
    }

    pub async fn sync_application(&self, app_name: &str) -> anyhow::Result<()> {
        let url = format!("{}/api/v1/applications/{app_name}/sync", self.api_base());
        let resp = self
            .http
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.token))
            .json(&serde_json::json!({}))
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("argocd sync failed for {app_name}: {status} — {body}");
        }

        tracing::info!(app = app_name, "argocd sync triggered");

        Ok(())
    }

    pub async fn sync_changed_apps(&self, changed_files: &[String]) {
        if changed_files.is_empty() {
            tracing::info!("no changed files, nothing to sync");
            return;
        }

        let apps = match self.list_applications().await {
            Ok(apps) => apps,
            Err(e) => {
                tracing::error!("failed to list argocd applications: {e}");
                return;
            }
        };

        let mut to_sync = BTreeSet::new();
        for app in &apps {
            let source_paths: Vec<String> = app
                .spec
                .all_sources()
                .filter(|s| {
                    s.chart.is_none()
                        && s.repo_url
                            .as_deref()
                            .is_some_and(|u| u.contains(MANIFEST_REPO))
                })
                .filter_map(|s| s.path.clone())
                .collect();
            if Self::app_affected(&app.metadata.name, &source_paths, changed_files) {
                to_sync.insert(app.metadata.name.clone());
            }
        }

        if to_sync.is_empty() {
            tracing::info!("no argocd applications affected by changed files, nothing to sync");
            return;
        }

        for app in to_sync {
            if let Err(e) = self.sync_application(&app).await {
                tracing::error!(app, "argocd sync failed: {e}");
            }
        }
    }

    fn api_base(&self) -> String {
        if self.server.starts_with("http://") || self.server.starts_with("https://") {
            self.server.trim_end_matches('/').to_string()
        } else {
            format!("https://{}", self.server.trim_end_matches('/'))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn affected(app: &str, paths: &[&str], files: &[&str]) -> bool {
        ArgocdClient::app_affected(
            app,
            &paths.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            &files.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        )
    }

    #[test]
    fn matches_file_under_source_path() {
        assert!(affected(
            "janitor-bot",
            &["projects/janitor-bot/manifests"],
            &["projects/janitor-bot/manifests/deployment.yaml"],
        ));
    }

    #[test]
    fn matches_when_source_path_differs_from_app_name() {
        // applications/envoy is the source path of the `envoy-gateway` app.
        assert!(affected(
            "envoy-gateway",
            &["applications/envoy", "gateway-helm"],
            &["applications/envoy/securitypolicy.yaml"],
        ));
    }

    #[test]
    fn matches_own_definition_file() {
        assert!(affected(
            "envoy-gateway",
            &["applications/envoy"],
            &["applications/envoy-gateway.application.yaml"],
        ));
        assert!(affected(
            "janitor-bot",
            &["projects/janitor-bot/manifests"],
            &["projects/janitor-bot/application.yaml"],
        ));
    }

    #[test]
    fn does_not_match_unrelated_files() {
        assert!(!affected(
            "janitor-bot",
            &["projects/janitor-bot/manifests"],
            &["terraform/main.tf", "applications/longhorn/foo.yaml"],
        ));
    }

    #[test]
    fn does_not_match_sibling_path_prefix() {
        // `applications/envoy` must not match `applications/envoy-gateway/...`.
        assert!(!affected(
            "envoy-gateway",
            &["applications/envoy"],
            &["applications/envoy-other/deployment.yaml"],
        ));
    }
}
