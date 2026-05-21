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
const REFRESH_CHECKBOX_CHECKED: &str = "- [x] Re-run diff on next poll";
const REFRESH_CHECKBOX_UNCHECKED: &str = "- [ ] Re-run diff on next poll";

/// Patterns in changed lines that are considered noise (Helm chart version bumps).
const NOISE_PATTERNS: &[&str] = &[
    "helm.sh/chart:",
    "app.kubernetes.io/version:",
    "chart:",
    "checksum/",
];

struct DiffFilterResult {
    diff: String,
    total_sections: usize,
    meaningful_sections: usize,
    filtered_sections: usize,
}

pub struct ArgocdClient {
    server: String,
    token: String,
    http: reqwest::Client,
}

impl ArgocdClient {
    pub fn new(server: String, token: String) -> Self {
        let http = reqwest::Client::builder()
            .danger_accept_invalid_certs(true)
            .timeout(std::time::Duration::from_secs(120))
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

    /// Filter noise from argocd diff output.
    ///
    /// The format uses `===== resource =====` section headers, with `<`/`>` for
    /// old/new lines and `NcN` or `N,NcN,N` range markers between hunks.
    /// We drop changed lines matching noise patterns and remove sections that
    /// become empty after filtering.
    fn filter_diff_noise(diff: &str) -> DiffFilterResult {
        use std::fmt::Write;

        fn is_noise(line: &str) -> bool {
            let content = line
                .strip_prefix("< ")
                .or_else(|| line.strip_prefix("> "))
                .unwrap_or(line)
                .trim();
            NOISE_PATTERNS.iter().any(|p| content.contains(p))
        }

        let mut result = String::new();
        let mut section_header: Option<&str> = None;
        let mut section_lines: Vec<&str> = Vec::new();
        let mut has_meaningful = false;
        let mut total_sections: usize = 0;
        let mut meaningful_sections: usize = 0;

        let flush = |result: &mut String,
                     header: Option<&str>,
                     lines: &[&str],
                     meaningful: bool,
                     meaningful_sections: &mut usize| {
            if !meaningful || header.is_none() {
                return;
            }
            *meaningful_sections += 1;
            let _ = writeln!(result, "{}", header.unwrap());
            for l in lines {
                let _ = writeln!(result, "{}", l);
            }
        };

        for line in diff.lines() {
            if line.starts_with("=====") {
                flush(
                    &mut result,
                    section_header,
                    &section_lines,
                    has_meaningful,
                    &mut meaningful_sections,
                );
                total_sections += 1;
                section_header = Some(line);
                section_lines.clear();
                has_meaningful = false;
            } else if section_header.is_some() {
                let is_change = line.starts_with("< ") || line.starts_with("> ");
                if is_change && is_noise(line) {
                    // skip noise line
                } else {
                    section_lines.push(line);
                    if is_change {
                        has_meaningful = true;
                    }
                }
            } else {
                let _ = writeln!(result, "{}", line);
            }
        }
        flush(
            &mut result,
            section_header,
            &section_lines,
            has_meaningful,
            &mut meaningful_sections,
        );

        let filtered = total_sections - meaningful_sections;
        let trimmed = result.trim().to_string();
        let diff = if trimmed.is_empty() {
            "No meaningful differences after filtering label/annotation noise.".to_string()
        } else {
            trimmed
        };

        DiffFilterResult {
            diff,
            total_sections,
            meaningful_sections,
            filtered_sections: filtered,
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

    fn format_comment(
        &self,
        source_diffs: &[SourceDiff],
        diff_results: &[DiffFilterResult],
    ) -> String {
        let mut s = format!("{}\n## ArgoCD Diff\n", *COMMENT_MARKER);

        for (sd, dr) in source_diffs.iter().zip(diff_results.iter()) {
            if dr.total_sections > 0 {
                s.push_str(&format!(
                    "\n> **{}** resources changed — **{}** meaningful, **{}** noise-only (filtered)\n",
                    dr.total_sections, dr.meaningful_sections, dr.filtered_sections,
                ));
            }

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
                dr.diff,
            ));
        }

        s.push_str(&format!("\n{REFRESH_CHECKBOX_UNCHECKED}\n"));
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

        // Skip if a diff comment already exists, unless the refresh checkbox is ticked.
        if let Some(existing) = client
            .find_bot_comment_with_marker_and_body(
                &pr.owner,
                &pr.repo,
                pr.pr_number as i64,
                &COMMENT_MARKER.to_string(),
            )
            .await?
        {
            if existing.body.contains(REFRESH_CHECKBOX_CHECKED) {
                tracing::info!(pr = pr.pr_number, "refresh checkbox ticked, re-running diff");
            } else {
                tracing::info!(
                    pr = pr.pr_number,
                    "argocd diff comment already exists, skipping"
                );
                return Ok(());
            }
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
            diff_outputs.push(Self::filter_diff_noise(&output));
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


    #[test]
    fn filter_all_noise_sections() {
        let diff = "\
===== /ConfigMap argocd/argocd-cm ======
137c137
<     helm.sh/chart: argo-cd-9.5.14
---
>     helm.sh/chart: argo-cd-9.5.15
===== /ConfigMap argocd/argocd-rbac-cm ======
27c27
<     helm.sh/chart: argo-cd-9.5.14
---
>     helm.sh/chart: argo-cd-9.5.15";

        let r = ArgocdClient::filter_diff_noise(diff);
        assert_eq!(
            r.diff,
            "No meaningful differences after filtering label/annotation noise."
        );
        assert_eq!(r.total_sections, 2);
        assert_eq!(r.meaningful_sections, 0);
        assert_eq!(r.filtered_sections, 2);
    }

    #[test]
    fn filter_keeps_meaningful_changes() {
        let diff = "\
===== apps/Deployment argocd/argocd-server ======
17c17
<     helm.sh/chart: argo-cd-9.5.14
---
>     helm.sh/chart: argo-cd-9.5.15
50c50
<     image: quay.io/argoproj/argocd:v2.13.0
---
>     image: quay.io/argoproj/argocd:v2.14.0";

        let r = ArgocdClient::filter_diff_noise(diff);
        assert!(r.diff.contains("image: quay.io/argoproj/argocd:v2.14.0"));
        assert!(!r.diff.contains("helm.sh/chart"));
        assert_eq!(r.total_sections, 1);
        assert_eq!(r.meaningful_sections, 1);
        assert_eq!(r.filtered_sections, 0);
    }

    #[test]
    fn filter_drops_checksum_noise() {
        let diff = "\
===== apps/Deployment argocd/argocd-dex-server ======
17c17
<     helm.sh/chart: argo-cd-9.5.14
---
>     helm.sh/chart: argo-cd-9.5.15
412,413c412,413
<         checksum/cm: d17e2338110a6b3f72df675b2d7b5bbfd46e9232822ca757ea7cd9018fb93707
<         checksum/cmd-params: a4bb04222c567f88501aaaf65a1a2bde08e4e423ecc0c2f0829e9d92a894b761
---
>         checksum/cm: 9831362e2f4b29e8ad863842e6e86b70b95857c57b70dc4ab0baad101a77da72
>         checksum/cmd-params: 589039711463031a7133fd3224e9b5b4d0bce2e03ed45d6738f3bba91dbff02b";

        let r = ArgocdClient::filter_diff_noise(diff);
        assert_eq!(
            r.diff,
            "No meaningful differences after filtering label/annotation noise."
        );
        assert_eq!(r.total_sections, 1);
        assert_eq!(r.filtered_sections, 1);
    }

    #[test]
    fn filter_mixed_noise_and_real() {
        let diff = "\
===== /ConfigMap argocd/argocd-cm ======
137c137
<     helm.sh/chart: argo-cd-9.5.14
---
>     helm.sh/chart: argo-cd-9.5.15
===== apps/Deployment argocd/argocd-server ======
17c17
<     helm.sh/chart: argo-cd-9.5.14
---
>     helm.sh/chart: argo-cd-9.5.15
50c50
<     replicas: 1
---
>     replicas: 2";

        let r = ArgocdClient::filter_diff_noise(diff);
        assert!(!r.diff.contains("ConfigMap"));
        assert!(r.diff.contains("Deployment"));
        assert!(r.diff.contains("replicas: 2"));
        assert!(!r.diff.contains("helm.sh/chart"));
        assert_eq!(r.total_sections, 2);
        assert_eq!(r.meaningful_sections, 1);
        assert_eq!(r.filtered_sections, 1);
    }

    #[test]
    fn filter_no_diff() {
        let r = ArgocdClient::filter_diff_noise("No differences found.");
        assert_eq!(r.diff, "No differences found.");
        assert_eq!(r.total_sections, 0);
    }

    #[test]
    fn filter_passthrough_no_noise() {
        let diff = "\
===== apps/Deployment default/myapp ======
10c10
<     replicas: 1
---
>     replicas: 3";

        let r = ArgocdClient::filter_diff_noise(diff);
        assert!(r.diff.contains("replicas: 3"));
        assert!(r.diff.contains("Deployment default/myapp"));
        assert_eq!(r.total_sections, 1);
        assert_eq!(r.meaningful_sections, 1);
        assert_eq!(r.filtered_sections, 0);
    }
}
