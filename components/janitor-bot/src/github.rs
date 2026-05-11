use crate::event::WorkflowEvent;
use hmac::{Hmac, Mac};
use serde::Deserialize;
use sha2::Sha256;

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

    pub async fn fetch_failed_jobs_summary(&self, jobs_url: &str) -> String {
        if jobs_url.is_empty() {
            return String::new();
        }

        let resp = match self
            .client
            .get(jobs_url)
            .header("Authorization", format!("Bearer {}", self.token))
            .header("Accept", "application/vnd.github+json")
            .header("User-Agent", "janitor-bot")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("failed to fetch jobs: {e}");
                return String::new();
            }
        };

        let jobs_resp: JobsResponse = match resp.json().await {
            Ok(j) => j,
            Err(e) => {
                tracing::warn!("failed to parse jobs response: {e}");
                return String::new();
            }
        };

        let mut summary = String::new();
        for job in &jobs_resp.jobs {
            let conclusion = job.conclusion.as_deref().unwrap_or("unknown");
            if conclusion != "failure" {
                continue;
            }
            let job_name = job.name.as_deref().unwrap_or("unknown");
            let job_url = job.html_url.as_deref().unwrap_or("N/A");

            summary.push_str(&format!("### :x: {}\n", job_name));
            summary.push_str(&format!("[View logs]({})\n", job_url));

            if let Some(steps) = &job.steps {
                let failed_steps: Vec<_> = steps
                    .iter()
                    .filter(|s| s.conclusion.as_deref() == Some("failure"))
                    .collect();

                if !failed_steps.is_empty() {
                    summary.push_str("\n| Step | # | Status |\n");
                    summary.push_str("|------|---|--------|\n");
                    for step in &failed_steps {
                        let step_name = step.name.as_deref().unwrap_or("unknown");
                        let step_num = step.number.unwrap_or(0);
                        summary.push_str(&format!(
                            "| {} | {} | `failure` |\n",
                            step_name, step_num
                        ));
                    }
                }
            }
            summary.push('\n');
        }

        summary
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
    number: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct Job {
    name: Option<String>,
    conclusion: Option<String>,
    html_url: Option<String>,
    steps: Option<Vec<JobStep>>,
}

#[derive(Debug, Deserialize)]
struct JobsResponse {
    jobs: Vec<Job>,
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
        failed_jobs_summary: String::new(),
    })
}
