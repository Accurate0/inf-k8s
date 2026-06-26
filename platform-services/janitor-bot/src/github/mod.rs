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
                logs.push_str(&raw_logs);
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

/// Computes the `X-Hub-Signature-256` header value for `body`, as GitHub sends
/// it: `sha256=<hex HMAC-SHA256(secret, body)>`. Inverse of [`verify_signature`];
/// used by the `janitor replay` tool to sign saved payloads.
pub fn sign_payload(secret: &str, body: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes())
        .expect("HMAC accepts keys of any size");
    mac.update(body);
    format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
}

/// Infers the `X-GitHub-Event` header from a raw webhook body by detecting
/// marker keys, mirroring the dispatch in `handle_github_webhook`. Returns
/// `None` if the shape matches none of the handled events.
pub fn infer_event(body: &[u8]) -> Option<&'static str> {
    let v: serde_json::Value = serde_json::from_slice(body).ok()?;
    let obj = v.as_object()?;

    if obj.get("workflow_run").is_some_and(|w| w.is_object()) {
        Some("workflow_run")
    } else if obj.get("check_run").is_some_and(|c| c.is_object()) {
        Some("check_run")
    } else if obj.contains_key("ref")
        && (obj.contains_key("pusher") || obj.contains_key("commits"))
    {
        Some("push")
    } else if obj.contains_key("context") && obj.contains_key("state") && obj.contains_key("sha") {
        Some("status")
    } else {
        None
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_payload_round_trips_with_verify() {
        let secret = "s3cr3t";
        let body = br#"{"hello":"world"}"#;
        let sig = sign_payload(secret, body);
        assert!(sig.starts_with("sha256="));
        assert!(verify_signature(secret, &sig, body));
        assert!(!verify_signature("wrong", &sig, body));
    }

    #[test]
    fn infer_event_detects_each_type() {
        assert_eq!(
            infer_event(br#"{"workflow_run":{"id":1},"action":"completed"}"#),
            Some("workflow_run")
        );
        assert_eq!(
            infer_event(br#"{"check_run":{"id":1},"action":"completed"}"#),
            Some("check_run")
        );
        assert_eq!(
            infer_event(br#"{"ref":"refs/heads/main","pusher":{"name":"x"}}"#),
            Some("push")
        );
        assert_eq!(
            infer_event(br#"{"sha":"abc","state":"failure","context":"ci/test"}"#),
            Some("status")
        );
        assert_eq!(infer_event(br#"{"unrelated":true}"#), None);
        assert_eq!(infer_event(b"not json"), None);
    }
}
