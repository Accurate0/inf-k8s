pub mod types;

use crate::event::PrEvent;
use crate::forgejo::ForgejoClient;
use std::process::Stdio;
use tokio::process::Command;
use types::{Application, SourceDiff};

const COMMENT_MARKER: &str = "<!-- janitor-bot:argocd-diff -->";
const MAX_DIFF_LEN: usize = 60_000;

pub struct ArgocdClient {
    server: String,
    token: String,
}

impl ArgocdClient {
    pub fn new(server: String, token: String) -> Self {
        Self { server, token }
    }

    pub fn from_env() -> anyhow::Result<Self> {
        let server = std::env::var("ARGOCD_SERVER")?;
        let token = std::env::var("ARGOCD_AUTH_TOKEN")?;
        Ok(Self { server, token })
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
        let mut s = format!("{COMMENT_MARKER}\n## ArgoCD Diff\n");

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
                COMMENT_MARKER,
                &comment,
            )
            .await?;

        Ok(())
    }
}
