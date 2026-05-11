use crate::event::WorkflowEvent;
use hmac::{Hmac, Mac};
use serde::Deserialize;
use sha2::Sha256;

#[derive(Default)]
pub struct FailedJobsResult {
    /// Raw log output from failed steps (no markdown)
    pub logs: String,
    /// Markdown-formatted links for each failed job
    pub links: String,
}

pub struct GitHubClient {
    client: reqwest::Client,
    token: String,
}

impl GitHubClient {
    pub fn from_env() -> anyhow::Result<Self> {
        let token = std::env::var("GITHUB_TOKEN")?;
        Ok(Self {
            client: reqwest::Client::new(),
            token,
        })
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
        let mut links = Vec::<String>::new();

        for job in &jobs_resp.jobs {
            let conclusion = job.conclusion.as_deref().unwrap_or("unknown");
            if conclusion != "failure" {
                continue;
            }
            let job_name = job.name.as_deref().unwrap_or("unknown");
            let job_url = job.html_url.as_deref().unwrap_or("");

            links.push(format!("- [{job_name}]({job_url})"));

            let failed_step_names: Vec<String> = job
                .steps
                .as_ref()
                .map(|steps| {
                    steps
                        .iter()
                        .filter(|s| s.conclusion.as_deref() == Some("failure"))
                        .filter_map(|s| s.name.clone())
                        .collect()
                })
                .unwrap_or_default();

            if let Some(job_id) = job.id {
                if let Some(raw_logs) = self.fetch_job_logs(jobs_url, job_id).await {
                    let filtered = extract_failed_step_logs(&raw_logs, &failed_step_names);
                    if !filtered.is_empty() {
                        if !logs.is_empty() {
                            logs.push('\n');
                        }
                        logs.push_str(&filtered);
                    }
                }
            }
        }

        FailedJobsResult {
            logs,
            links: links.join("\n"),
        }
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

#[derive(Debug, Deserialize)]
struct CommitAuthor {
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct HeadCommit {
    #[allow(dead_code)]
    id: Option<String>,
    message: Option<String>,
    author: Option<CommitAuthor>,
}

#[derive(Debug, Deserialize)]
struct Actor {
    login: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WorkflowRun {
    name: Option<String>,
    conclusion: Option<String>,
    html_url: Option<String>,
    head_branch: Option<String>,
    head_sha: Option<String>,
    head_commit: Option<HeadCommit>,
    actor: Option<Actor>,
    run_number: Option<u64>,
    run_attempt: Option<u64>,
    jobs_url: Option<String>,
    display_title: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WorkflowRepository {
    full_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WorkflowRunPayload {
    workflow_run: Option<WorkflowRun>,
    repository: Option<WorkflowRepository>,
}

#[derive(Debug, Deserialize)]
struct JobStep {
    name: Option<String>,
    conclusion: Option<String>,
    #[allow(dead_code)]
    number: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct Job {
    id: Option<u64>,
    name: Option<String>,
    conclusion: Option<String>,
    html_url: Option<String>,
    steps: Option<Vec<JobStep>>,
}

#[derive(Debug, Deserialize)]
struct JobsResponse {
    jobs: Vec<Job>,
}

/// Extract log lines belonging to failed steps from raw GitHub Actions job logs.
///
/// GitHub Actions logs have lines like:
///   `2024-01-01T00:00:00.0000000Z ##[group]Step Name`
/// to mark the start of a step, and `##[endgroup]` to end it.
/// Lines within a step are prefixed with a timestamp.
fn extract_failed_step_logs(raw_logs: &str, failed_step_names: &[String]) -> String {
    let mut result = String::new();
    let mut capturing = false;
    for line in raw_logs.lines() {
        // Strip the timestamp prefix (everything up to and including the first space after the Z)
        let content = line
            .find("Z ")
            .map(|i| &line[i + 2..])
            .unwrap_or(line);

        if let Some(group_name) = content.strip_prefix("##[group]") {
            let step_name = group_name.trim();
            if failed_step_names.iter().any(|n| n == step_name) {
                capturing = true;
                if !result.is_empty() {
                    result.push('\n');
                }
                result.push_str(&format!("=== {step_name} ===\n"));
            } else {
                capturing = false;
            }
            continue;
        }

        if content.starts_with("##[endgroup]") {
            capturing = false;
            continue;
        }

        if capturing {
            // Strip ##[error] prefixes for cleaner output
            let clean = content
                .strip_prefix("##[error]")
                .unwrap_or(content);
            result.push_str(clean);
            result.push('\n');
        }
    }

    // Truncate to avoid creating excessively large issues
    const MAX_LOG_BYTES: usize = 50_000;
    if result.len() > MAX_LOG_BYTES {
        result.truncate(MAX_LOG_BYTES);
        result.push_str("\n... (truncated)\n");
    }

    result
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
        workflow_name: run.name?,
        conclusion: run.conclusion.unwrap_or_default(),
        run_url: run.html_url?,
        repository: payload.repository?.full_name?,
        branch: run.head_branch.unwrap_or_default(),
        head_sha: run.head_sha.unwrap_or_default(),
        commit_message: head_commit.message.unwrap_or_default(),
        commit_author: head_commit
            .author
            .and_then(|a| a.name)
            .unwrap_or_default(),
        actor: run.actor.and_then(|a| a.login).unwrap_or_default(),
        run_number: run.run_number.unwrap_or(0),
        run_attempt: run.run_attempt.unwrap_or(1),
        jobs_url: run.jobs_url.unwrap_or_default(),
        display_title: run.display_title.unwrap_or_default(),
        failed_jobs_logs: String::new(),
        failed_jobs_links: String::new(),
    })
}
