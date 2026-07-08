use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct PrBase {
    #[serde(rename = "ref")]
    pub r#ref: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct PullRequest {
    pub number: u64,
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub labels: Vec<Label>,
    pub base: Option<PrBase>,
    #[serde(default)]
    pub merged: bool,
    #[serde(default)]
    pub merge_commit_sha: Option<String>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct Label {
    pub id: u64,
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct User {
    pub login: String,
}

#[derive(Debug, Deserialize)]
pub struct Repository {
    pub full_name: String,
}

#[derive(Debug, Deserialize)]
pub struct WebhookEvent {
    pub action: String,
    pub pull_request: Option<PullRequest>,
    pub issue: Option<IssueRef>,
    pub comment: Option<Comment>,
    pub sender: Option<User>,
    pub repository: Option<Repository>,
}

#[derive(Debug, Deserialize)]
pub struct Comment {
    pub id: i64,
    pub body: String,
}

#[derive(Debug, Deserialize)]
pub struct IssueRef {
    pub number: u64,
    #[serde(default)]
    pub pull_request: Option<serde_json::Value>,
    pub body: Option<String>,
    #[serde(default)]
    pub labels: Vec<Label>,
}

#[derive(Debug)]
pub struct CommentEvent {
    pub owner: String,
    pub repo: String,
    pub pr_number: u64,
    pub comment_id: i64,
    pub author: String,
    pub body: String,
}

#[derive(Debug)]
pub struct IssueCommentEvent {
    pub owner: String,
    pub repo: String,
    pub issue_number: u64,
    pub comment_id: i64,
    pub author: String,
    pub comment_body: String,
    pub issue_body: String,
    pub issue_labels: Vec<String>,
}

#[allow(dead_code)]
pub struct PrEvent {
    pub action: String,
    pub author: String,
    pub owner: String,
    pub repo: String,
    pub pr_number: u64,
    pub title: String,
    pub body: String,
    pub target_branch: String,
    pub labels: Vec<Label>,
    pub merged: bool,
    pub merge_commit_sha: Option<String>,
}

#[allow(dead_code)]
pub struct CommitStatusEvent {
    pub repository: String,
    pub sha: String,
    pub state: String,
    pub context: String,
    pub description: String,
    pub target_url: String,
}

#[allow(dead_code)]
pub struct ArgoSyncEvent {
    pub app_name: String,
    pub sha: String,
    pub sync_status: String,
    pub health_status: String,
    pub phase: String,
    pub message: String,
}

/// A GitHub `push` webhook, parsed only enough for rule matching
/// (`repository`, `branch`). Forwarding the original request is handled
/// generically by the `proxy_pass` action via [`RawRequest`], so the raw
/// bytes are intentionally *not* stored here.
#[allow(dead_code)]
pub struct PushEvent {
    pub repository: String,
    pub branch: String,
}

/// The original inbound HTTP request (body + headers) that triggered an
/// evaluation. Captured at the webhook boundary and threaded through to action
/// execution so the generic `proxy_pass` action can forward it verbatim —
/// preserving the GitHub signature so the downstream service can re-verify it.
#[derive(Clone, Default)]
pub struct RawRequest {
    pub body: Vec<u8>,
    pub headers: Vec<(String, String)>,
}

#[allow(dead_code)]
pub struct CheckRunEvent {
    pub repository: String,
    pub sha: String,
    pub name: String,
    pub status: String,
    pub conclusion: String,
    pub details_url: String,
    pub app_name: String,
    pub run_id: Option<u64>,
    pub workflow_name: String,
}

#[allow(dead_code)]
pub struct WorkflowEvent {
    pub run_id: u64,
    pub workflow_name: String,
    pub conclusion: String,
    pub run_url: String,
    pub repository: String,
    pub branch: String,
    pub head_sha: String,
    pub commit_message: String,
    pub commit_author: String,
    pub actor: String,
    pub run_number: u64,
    pub run_attempt: u64,
    pub jobs_url: String,
    pub display_title: String,
    pub failed_jobs_logs: String,
    pub created_at: String,
    pub updated_at: String,
}

pub enum BotEvent<'a> {
    ForgejoPr(&'a PrEvent),
    GitHubWorkflow(&'a WorkflowEvent),
    GitHubCommitStatus(&'a CommitStatusEvent),
    GitHubCheckRun(&'a CheckRunEvent),
    GitHubPush(&'a PushEvent),
    ArgoSync(&'a ArgoSyncEvent),
}
