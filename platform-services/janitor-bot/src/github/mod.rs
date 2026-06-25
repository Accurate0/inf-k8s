pub mod types;

use crate::event::WorkflowEvent;
use hmac::{Hmac, KeyInit, Mac};
use serde::Deserialize;
use sha2::Sha256;
use types::{
    CheckRunPayload, CommitStatusPayload, HeadCommit, JobsResponse, PushPayload, WorkflowRunPayload,
};

pub use types::FailedJobsResult;

pub struct GitHubClient {
    client: reqwest::Client,
    token: String,
    base_url: String,
}

impl GitHubClient {
    pub fn from_env() -> anyhow::Result<Self> {
        let token = std::env::var("GITHUB_TOKEN")?;
        let base_url =
            std::env::var("GITHUB_URL").unwrap_or_else(|_| "https://api.github.com".to_string());

        Ok(Self::new(base_url, token))
    }

    pub fn new(base_url: String, token: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            token,
            base_url,
        }
    }

    async fn github_get(&self, url: &str) -> Result<reqwest::Response, reqwest::Error> {
        self.client
            .get(url)
            .header("Authorization", format!("Bearer {}", self.token))
            .header("Accept", "application/vnd.github+json")
            .header("User-Agent", "janitor-bot")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .send()
            .await
    }

    #[tracing::instrument(skip_all)]
    pub async fn health_check(&self) -> anyhow::Result<()> {
        self.github_get(&format!("{}/user", self.base_url))
            .await?
            .error_for_status()?;
        Ok(())
    }

    async fn fetch_job_logs(&self, jobs_url: &str, job_id: u64) -> Option<String> {
        // jobs_url: https://api.github.com/repos/{owner}/{repo}/actions/runs/{run_id}/jobs
        // logs_url: https://api.github.com/repos/{owner}/{repo}/actions/jobs/{job_id}/logs
        let base = jobs_url.split("/actions/runs/").next()?;
        let logs_url = format!("{base}/actions/jobs/{job_id}/logs");

        match self.github_get(&logs_url).await {
            Ok(resp) => match resp.text().await {
                Ok(text) => Some(text),
                Err(e) => {
                    tracing::warn!(job_id, "failed to read job logs body: {e}");
                    None
                }
            },
            Err(e) => {
                tracing::warn!(job_id, "failed to fetch job logs: {e}");
                None
            }
        }
    }

    #[tracing::instrument(skip_all)]
    pub async fn rerun_workflow(&self, owner: &str, repo: &str, run_id: u64) -> anyhow::Result<()> {
        let url = format!(
            "{}/repos/{owner}/{repo}/actions/runs/{run_id}/rerun-failed-jobs",
            self.base_url
        );
        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.token))
            .header("Accept", "application/vnd.github+json")
            .header("User-Agent", "janitor-bot")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("rerun failed: {status} — {body}");
        }

        tracing::info!(owner, repo, run_id, "rerun workflow triggered");

        Ok(())
    }

    /// Returns whether `sha` is available as a commit on `owner/repo`.
    #[tracing::instrument(skip_all)]
    pub async fn commit_exists(&self, owner: &str, repo: &str, sha: &str) -> bool {
        let url = format!("{}/repos/{owner}/{repo}/commits/{sha}", self.base_url);
        match self.github_get(&url).await {
            Ok(resp) => resp.status().is_success(),
            Err(e) => {
                tracing::warn!(owner, repo, sha, "failed to check commit: {e}");
                false
            }
        }
    }

    #[tracing::instrument(skip_all)]
    pub async fn workflow_run_name(&self, owner: &str, repo: &str, run_id: u64) -> Option<String> {
        let url = format!(
            "{}/repos/{owner}/{repo}/actions/runs/{run_id}",
            self.base_url
        );
        match self.github_get(&url).await {
            Ok(resp) => {
                let resp = resp.error_for_status().ok()?;
                #[derive(Deserialize)]
                struct WorkflowRun {
                    name: Option<String>,
                }
                resp.json::<WorkflowRun>().await.ok()?.name
            }
            Err(e) => {
                tracing::warn!(owner, repo, run_id, "failed to fetch workflow run: {e}");
                None
            }
        }
    }

    /// Fetches failed-job logs for a workflow run identified by its API base
    /// (`owner`/`repo`/`run_id`), reusing [`fetch_failed_jobs`].
    #[tracing::instrument(skip_all)]
    pub async fn failed_logs_for_run(
        &self,
        owner: &str,
        repo: &str,
        run_id: u64,
    ) -> FailedJobsResult {
        let jobs_url = format!(
            "{}/repos/{owner}/{repo}/actions/runs/{run_id}/jobs",
            self.base_url
        );
        self.fetch_failed_jobs(&jobs_url).await
    }

    #[tracing::instrument(skip_all)]
    pub async fn fetch_failed_jobs(&self, jobs_url: &str) -> FailedJobsResult {
        let empty = FailedJobsResult::default();

        if jobs_url.is_empty() {
            return empty;
        }

        let resp = match self.github_get(jobs_url).await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("failed to fetch jobs: {e}");
                return empty;
            }
        };

        let jobs_resp: JobsResponse = match resp.json().await {
            Ok(j) => j,
            Err(e) => {
                tracing::warn!("failed to parse jobs response: {e}");
                return empty;
            }
        };

        let mut logs = String::new();

        for job in &jobs_resp.jobs {
            let conclusion = job.conclusion.as_deref().unwrap_or("unknown");
            if conclusion != "failure" {
                continue;
            }

            if let Some(job_id) = job.id
                && let Some(raw_logs) = self.fetch_job_logs(jobs_url, job_id).await
            {
                let filtered = extract_error_lines(&raw_logs);
                if !filtered.is_empty() {
                    if !logs.is_empty() {
                        logs.push('\n');
                    }
                    logs.push_str(&filtered);
                }
            }
        }

        let logs = logs.trim_end_matches('\n').to_string();

        FailedJobsResult { logs }
    }
}

pub fn verify_signature(secret: &str, signature: &str, body: &[u8]) -> bool {
    let Some(hex_sig) = signature.strip_prefix("sha256=") else {
        return false;
    };
    let Ok(decoded) = hex::decode(hex_sig) else {
        return false;
    };
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(secret.as_bytes()) else {
        return false;
    };

    mac.update(body);

    mac.verify_slice(&decoded).is_ok()
}

/// Extract error lines from raw GitHub Actions job logs.
///
/// Looks for `##[error]` annotations and includes surrounding context lines.
fn extract_error_lines(raw_logs: &str) -> String {
    let lines: Vec<&str> = raw_logs.lines().collect();

    // Strip timestamp prefix from a log line
    fn strip_ts(line: &str) -> &str {
        line.find("Z ").map(|i| &line[i + 2..]).unwrap_or(line)
    }

    // Find indices of error lines
    let error_indices: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| strip_ts(line).starts_with("##[error]"))
        .map(|(i, _)| i)
        .collect();

    if error_indices.is_empty() {
        return String::new();
    }

    const CONTEXT: usize = 3;
    let mut result = String::new();
    let mut last_printed = None::<usize>;

    for &idx in &error_indices {
        let start = idx.saturating_sub(CONTEXT);
        let end = (idx + CONTEXT + 1).min(lines.len());

        // Add separator if there's a gap from the last printed range
        if let Some(last) = last_printed
            && start > last + 1
            && !result.is_empty()
        {
            result.push_str("...\n");
        }

        for (i, line) in lines.iter().enumerate().take(end).skip(start) {
            if let Some(last) = last_printed
                && i <= last
            {
                continue;
            }
            let content = strip_ts(line);
            // Skip group markers
            if content.starts_with("##[group]") || content.starts_with("##[endgroup]") {
                continue;
            }
            let clean = content.strip_prefix("##[error]").unwrap_or(content);
            result.push_str(clean);
            result.push('\n');
            last_printed = Some(i);
        }
    }

    const MAX_LOG_BYTES: usize = 50_000;
    if result.len() > MAX_LOG_BYTES {
        result.truncate(MAX_LOG_BYTES);
        result.push_str("\n... (truncated)\n");
    }

    result
}

fn extract_run_id(url: &str) -> Option<u64> {
    url.split("/actions/runs/")
        .nth(1)?
        .split('/')
        .next()?
        .parse()
        .ok()
}

pub fn parse_check_run_event(body: &[u8]) -> Option<crate::event::CheckRunEvent> {
    let payload: CheckRunPayload = serde_json::from_slice(body).ok()?;
    let action = payload.action?;

    if action != "completed" && action != "created" {
        return None;
    }

    let cr = payload.check_run?;
    let details_url = cr.details_url.unwrap_or_default();
    let run_id = extract_run_id(&details_url);

    Some(crate::event::CheckRunEvent {
        repository: payload.repository?.full_name?,
        sha: cr.head_sha?,
        name: cr.name.unwrap_or_default(),
        status: cr.status.unwrap_or_default(),
        conclusion: cr.conclusion.unwrap_or_default(),
        details_url,
        app_name: cr.app.and_then(|a| a.name).unwrap_or_default(),
        run_id,
        workflow_name: String::new(),
    })
}

pub fn parse_push_event(body: &[u8]) -> Option<crate::event::PushEvent> {
    let payload: PushPayload = serde_json::from_slice(body).ok()?;
    let branch = payload
        .r#ref
        .as_deref()
        .and_then(|r| r.strip_prefix("refs/heads/"))
        .unwrap_or_default()
        .to_string();

    Some(crate::event::PushEvent {
        repository: payload.repository?.full_name?,
        branch,
    })
}

pub fn parse_commit_status_event(body: &[u8]) -> Option<crate::event::CommitStatusEvent> {
    let payload: CommitStatusPayload = serde_json::from_slice(body).ok()?;
    Some(crate::event::CommitStatusEvent {
        repository: payload.repository?.full_name?,
        sha: payload.sha?,
        state: payload.state?,
        context: payload.context.unwrap_or_default(),
        description: payload.description.unwrap_or_default(),
        target_url: payload.target_url.unwrap_or_default(),
    })
}

pub fn parse_workflow_event(body: &[u8]) -> Option<WorkflowEvent> {
    let payload: WorkflowRunPayload = serde_json::from_slice(body).ok()?;
    let run = payload.workflow_run?;
    let head_commit = run.head_commit.unwrap_or(HeadCommit {
        id: None,
        message: None,
        author: None,
    });

    Some(WorkflowEvent {
        run_id: run.id?,
        workflow_name: run.name?,
        conclusion: run.conclusion.unwrap_or_default(),
        run_url: run.html_url?,
        repository: payload.repository?.full_name?,
        branch: run.head_branch.unwrap_or_default(),
        head_sha: run.head_sha.unwrap_or_default(),
        commit_message: head_commit.message.unwrap_or_default(),
        commit_author: head_commit.author.and_then(|a| a.name).unwrap_or_default(),
        actor: run.actor.and_then(|a| a.login).unwrap_or_default(),
        run_number: run.run_number.unwrap_or(0),
        run_attempt: run.run_attempt.unwrap_or(1),
        jobs_url: run.jobs_url.unwrap_or_default(),
        display_title: run.display_title.unwrap_or_default(),
        failed_jobs_logs: String::new(),
        created_at: run.created_at.unwrap_or_default(),
        updated_at: run.updated_at.unwrap_or_default(),
    })
}
