use serde::Deserialize;

#[derive(Default)]
pub struct FailedJobsResult {
    pub logs: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct CommitAuthor {
    pub name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct HeadCommit {
    #[allow(dead_code)]
    pub id: Option<String>,
    pub message: Option<String>,
    pub author: Option<CommitAuthor>,
}

#[derive(Debug, Deserialize)]
pub(super) struct Actor {
    pub login: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct WorkflowRun {
    pub id: Option<u64>,
    pub name: Option<String>,
    pub conclusion: Option<String>,
    pub html_url: Option<String>,
    pub head_branch: Option<String>,
    pub head_sha: Option<String>,
    pub head_commit: Option<HeadCommit>,
    pub actor: Option<Actor>,
    pub run_number: Option<u64>,
    pub run_attempt: Option<u64>,
    pub jobs_url: Option<String>,
    pub display_title: Option<String>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct WorkflowRepository {
    pub full_name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct WorkflowRunPayload {
    pub workflow_run: Option<WorkflowRun>,
    pub repository: Option<WorkflowRepository>,
}

#[derive(Debug, Deserialize)]
pub(super) struct Job {
    pub id: Option<u64>,
    pub conclusion: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct JobsResponse {
    pub jobs: Vec<Job>,
}

#[derive(Debug, Deserialize)]
pub(super) struct CommitStatusRepository {
    pub full_name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct CommitStatusPayload {
    pub sha: Option<String>,
    pub state: Option<String>,
    pub context: Option<String>,
    pub description: Option<String>,
    pub target_url: Option<String>,
    pub repository: Option<CommitStatusRepository>,
}

#[derive(Debug, Deserialize)]
pub(super) struct CheckRunApp {
    pub name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct CheckRun {
    pub name: Option<String>,
    pub head_sha: Option<String>,
    pub status: Option<String>,
    pub conclusion: Option<String>,
    pub details_url: Option<String>,
    pub app: Option<CheckRunApp>,
}

#[derive(Debug, Deserialize)]
pub(super) struct CheckRunRepository {
    pub full_name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct CheckRunPayload {
    pub action: Option<String>,
    pub check_run: Option<CheckRun>,
    pub repository: Option<CheckRunRepository>,
}
