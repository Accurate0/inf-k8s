use serde::Deserialize;
use std::collections::HashMap;

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
    pub labels: Vec<Label>,
    pub base: Option<PrBase>,
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

impl WebhookEvent {
    pub fn into_issue_comment_event(self) -> Option<IssueCommentEvent> {
        let comment = self.comment?;
        let issue = self.issue?;
        let sender = self.sender?;
        let repository = self.repository?;
        let (owner, repo) = repository.full_name.split_once('/')?;
        Some(IssueCommentEvent {
            owner: owner.to_owned(),
            repo: repo.to_owned(),
            issue_number: issue.number,
            comment_id: comment.id,
            author: sender.login,
            comment_body: comment.body,
            issue_body: issue.body.unwrap_or_default(),
            issue_labels: issue.labels.into_iter().map(|l| l.name).collect(),
        })
    }

    pub fn into_comment_event(self) -> Option<CommentEvent> {
        let comment = self.comment?;
        let issue = self.issue?;
        let sender = self.sender?;
        let repository = self.repository?;
        let (owner, repo) = repository.full_name.split_once('/')?;
        Some(CommentEvent {
            owner: owner.to_owned(),
            repo: repo.to_owned(),
            pr_number: issue.number,
            comment_id: comment.id,
            author: sender.login,
            body: comment.body,
        })
    }
}

#[allow(dead_code)]
pub struct PrEvent {
    pub action: String,
    pub author: String,
    pub owner: String,
    pub repo: String,
    pub pr_number: u64,
    pub title: String,
    pub target_branch: String,
    pub labels: Vec<Label>,
    pub changed_files: Vec<String>,
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

#[allow(dead_code)]
pub struct CheckRunEvent {
    pub repository: String,
    pub sha: String,
    pub name: String,
    pub status: String,
    pub conclusion: String,
    pub details_url: String,
    pub app_name: String,
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
    ArgoSync(&'a ArgoSyncEvent),
}

impl BotEvent<'_> {
    pub fn template_vars(&self) -> HashMap<&'static str, String> {
        let mut vars = HashMap::new();
        match self {
            BotEvent::ForgejoPr(pr) => {
                vars.insert("action", pr.action.clone());
                vars.insert("author", pr.author.clone());
                vars.insert("owner", pr.owner.clone());
                vars.insert("repo", pr.repo.clone());
                vars.insert("pr_number", pr.pr_number.to_string());
                vars.insert("title", pr.title.clone());
                vars.insert("target_branch", pr.target_branch.clone());
            }
            BotEvent::GitHubCommitStatus(cs) => {
                vars.insert("repository", cs.repository.clone());
                vars.insert("sha", cs.sha.clone());
                vars.insert("state", cs.state.clone());
                vars.insert("context", cs.context.clone());
                vars.insert("description", cs.description.clone());
                vars.insert("target_url", cs.target_url.clone());
                let short_sha = if cs.sha.len() >= 7 {
                    &cs.sha[..7]
                } else {
                    &cs.sha
                };
                vars.insert("short_sha", short_sha.to_string());
            }
            BotEvent::ArgoSync(sync) => {
                vars.insert("app_name", sync.app_name.clone());
                vars.insert("sha", sync.sha.clone());
                vars.insert("sync_status", sync.sync_status.clone());
                vars.insert("health_status", sync.health_status.clone());
                vars.insert("phase", sync.phase.clone());
                vars.insert("message", sync.message.clone());
                let state = match sync.phase.as_str() {
                    "Succeeded" => match sync.health_status.as_str() {
                        "Healthy" => "success",
                        "Degraded" => "failure",
                        _ => "pending",
                    },
                    "Failed" | "Error" => "failure",
                    "Running" => "pending",
                    _ => "pending",
                };
                vars.insert("state", state.to_string());
                vars.insert(
                    "context",
                    format!("ArgoCD / sync / {}", sync.app_name),
                );
                vars.insert("description", format!("{} - {}", sync.phase, sync.health_status));
                vars.insert("target_url", String::new());
                let short_sha = if sync.sha.len() >= 7 {
                    &sync.sha[..7]
                } else {
                    &sync.sha
                };
                vars.insert("short_sha", short_sha.to_string());
            }
            BotEvent::GitHubCheckRun(cr) => {
                vars.insert("repository", cr.repository.clone());
                vars.insert("sha", cr.sha.clone());
                vars.insert("name", cr.name.clone());
                vars.insert("status", cr.status.clone());
                vars.insert("conclusion", cr.conclusion.clone());
                vars.insert("details_url", cr.details_url.clone());
                vars.insert("app_name", cr.app_name.clone());
                // Map check_run conclusion to commit status state
                let state = match cr.conclusion.as_str() {
                    "success" => "success",
                    "failure" | "timed_out" | "action_required" => "failure",
                    "cancelled" | "skipped" | "stale" => "error",
                    "neutral" => "success",
                    _ if cr.status == "in_progress" || cr.status == "queued" => "pending",
                    _ => "pending",
                };
                vars.insert("state", state.to_string());
                // context and description for compatibility with set_commit_status action
                let context = if cr.app_name.is_empty() {
                    cr.name.clone()
                } else {
                    format!("{} / {}", cr.app_name, cr.name)
                };
                vars.insert("context", context);
                vars.insert("description", cr.conclusion.clone());
                vars.insert("target_url", cr.details_url.clone());
                let short_sha = if cr.sha.len() >= 7 {
                    &cr.sha[..7]
                } else {
                    &cr.sha
                };
                vars.insert("short_sha", short_sha.to_string());
            }
            BotEvent::GitHubWorkflow(wf) => {
                vars.insert("run_id", wf.run_id.to_string());
                vars.insert("workflow_name", wf.workflow_name.clone());
                vars.insert("conclusion", wf.conclusion.clone());
                vars.insert("run_url", wf.run_url.clone());
                vars.insert("repository", wf.repository.clone());
                vars.insert("branch", wf.branch.clone());
                vars.insert("head_sha", wf.head_sha.clone());
                vars.insert("commit_message", wf.commit_message.clone());
                vars.insert("commit_author", wf.commit_author.clone());
                vars.insert("actor", wf.actor.clone());
                vars.insert("run_number", wf.run_number.to_string());
                vars.insert("run_attempt", wf.run_attempt.to_string());
                vars.insert("display_title", wf.display_title.clone());
                vars.insert("failed_jobs_logs", wf.failed_jobs_logs.clone());
                vars.insert("created_at", wf.created_at.clone());
                vars.insert("updated_at", wf.updated_at.clone());
                let logs_url = format!("{}/logs", wf.run_url);
                vars.insert("logs_url", logs_url.clone());
                let metadata_json = serde_json::json!({
                    "run_id": wf.run_id,
                    "workflow": wf.workflow_name,
                    "conclusion": wf.conclusion,
                    "branch": wf.branch,
                    "commit_sha": wf.head_sha,
                    "created_at": wf.created_at,
                    "updated_at": wf.updated_at,
                    "html_url": wf.run_url,
                    "logs_url": logs_url,
                });
                vars.insert("metadata_json", metadata_json.to_string());
                let short_sha = if wf.head_sha.len() >= 7 {
                    &wf.head_sha[..7]
                } else {
                    &wf.head_sha
                };
                vars.insert("short_sha", short_sha.to_string());
            }
        }
        vars
    }
}

pub fn render_template(template: &str, vars: &HashMap<&str, String>) -> String {
    let mut result = template.to_owned();
    for (key, value) in vars {
        result = result.replace(&format!("{{{key}}}"), value);
    }
    result
}

impl PrEvent {
    pub fn from_api_pr(
        pr: &forgejo_api::structs::PullRequest,
        owner: String,
        repo: String,
    ) -> Option<Self> {
        Some(PrEvent {
            action: "opened".into(),
            author: pr.user.as_ref()?.login.clone()?,
            owner,
            repo,
            pr_number: pr.number? as u64,
            title: pr.title.clone()?,
            target_branch: pr.base.as_ref()?.r#ref.clone().unwrap_or_default(),
            labels: pr
                .labels
                .as_ref()
                .map(|ls| {
                    ls.iter()
                        .filter_map(|l| {
                            Some(Label {
                                id: l.id? as u64,
                                name: l.name.clone()?,
                            })
                        })
                        .collect()
                })
                .unwrap_or_default(),
            changed_files: Vec::new(),
        })
    }
}

impl WebhookEvent {
    pub fn into_pr_event(self) -> Option<PrEvent> {
        let pr = self.pull_request?;
        let sender = self.sender?;
        let repository = self.repository?;
        let (owner, repo) = repository.full_name.split_once('/')?;
        let owner = owner.to_owned();
        let repo = repo.to_owned();
        Some(PrEvent {
            action: self.action,
            author: sender.login,
            owner,
            repo,
            pr_number: pr.number,
            title: pr.title,
            target_branch: pr.base.and_then(|b| b.r#ref).unwrap_or_default(),
            labels: pr.labels,
            changed_files: Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_template_basic() {
        let mut vars = HashMap::new();
        vars.insert("name", "world".to_string());
        assert_eq!(render_template("hello {name}", &vars), "hello world");
    }

    #[test]
    fn render_template_multiple_vars() {
        let mut vars = HashMap::new();
        vars.insert("a", "1".to_string());
        vars.insert("b", "2".to_string());
        assert_eq!(render_template("{a} + {b}", &vars), "1 + 2");
    }

    #[test]
    fn render_template_repeated_var() {
        let mut vars = HashMap::new();
        vars.insert("x", "hi".to_string());
        assert_eq!(render_template("{x} and {x}", &vars), "hi and hi");
    }

    #[test]
    fn render_template_no_vars() {
        let vars = HashMap::new();
        assert_eq!(render_template("no placeholders", &vars), "no placeholders");
    }

    #[test]
    fn render_template_missing_var_left_as_is() {
        let vars = HashMap::new();
        assert_eq!(render_template("{unknown}", &vars), "{unknown}");
    }

    #[test]
    fn render_template_empty_template() {
        let vars = HashMap::new();
        assert_eq!(render_template("", &vars), "");
    }

    fn make_pr_event() -> PrEvent {
        PrEvent {
            action: "opened".to_string(),
            author: "renovate".to_string(),
            owner: "anurag".to_string(),
            repo: "k8s".to_string(),
            pr_number: 42,
            title: "bump stuff".to_string(),
            target_branch: "main".to_string(),
            labels: vec![],
            changed_files: vec![],
        }
    }

    fn make_workflow_event() -> WorkflowEvent {
        WorkflowEvent {
            run_id: 123,
            workflow_name: "build".to_string(),
            conclusion: "failure".to_string(),
            run_url: "https://github.com/org/repo/actions/runs/123".to_string(),
            repository: "org/repo".to_string(),
            branch: "main".to_string(),
            head_sha: "abcdef1234567890".to_string(),
            commit_message: "fix stuff".to_string(),
            commit_author: "dev".to_string(),
            actor: "dev".to_string(),
            run_number: 5,
            run_attempt: 1,
            jobs_url: "https://api.github.com/repos/org/repo/actions/runs/123/jobs".to_string(),
            display_title: "build".to_string(),
            failed_jobs_logs: "error: something broke".to_string(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:01:00Z".to_string(),
        }
    }

    #[test]
    fn template_vars_pr_event() {
        let pr = make_pr_event();
        let event = BotEvent::ForgejoPr(&pr);
        let vars = event.template_vars();
        assert_eq!(vars["action"], "opened");
        assert_eq!(vars["author"], "renovate");
        assert_eq!(vars["owner"], "anurag");
        assert_eq!(vars["repo"], "k8s");
        assert_eq!(vars["pr_number"], "42");
        assert_eq!(vars["title"], "bump stuff");
        assert_eq!(vars["target_branch"], "main");
    }

    #[test]
    fn template_vars_workflow_event() {
        let wf = make_workflow_event();
        let event = BotEvent::GitHubWorkflow(&wf);
        let vars = event.template_vars();
        assert_eq!(vars["run_id"], "123");
        assert_eq!(vars["workflow_name"], "build");
        assert_eq!(vars["conclusion"], "failure");
        assert_eq!(vars["branch"], "main");
        assert_eq!(vars["run_number"], "5");
        assert_eq!(vars["run_attempt"], "1");
        assert_eq!(vars["short_sha"], "abcdef1");
        assert_eq!(vars["commit_message"], "fix stuff");
        assert_eq!(vars["commit_author"], "dev");
        assert!(vars.contains_key("metadata_json"));
        assert!(vars.contains_key("logs_url"));
    }

    #[test]
    fn template_vars_short_sha_truncates() {
        let wf = make_workflow_event();
        let event = BotEvent::GitHubWorkflow(&wf);
        let vars = event.template_vars();
        assert_eq!(vars["short_sha"].len(), 7);
    }

    #[test]
    fn template_vars_short_sha_short_input() {
        let mut wf = make_workflow_event();
        wf.head_sha = "abc".to_string();
        let event = BotEvent::GitHubWorkflow(&wf);
        let vars = event.template_vars();
        assert_eq!(vars["short_sha"], "abc");
    }

    #[test]
    fn template_vars_metadata_json_is_valid() {
        let wf = make_workflow_event();
        let event = BotEvent::GitHubWorkflow(&wf);
        let vars = event.template_vars();
        let parsed: serde_json::Value = serde_json::from_str(&vars["metadata_json"]).unwrap();
        assert_eq!(parsed["run_id"], 123);
        assert_eq!(parsed["workflow"], "build");
        assert_eq!(parsed["conclusion"], "failure");
    }

    #[test]
    fn webhook_event_into_pr_event() {
        let wh = WebhookEvent {
            action: "opened".to_string(),
            pull_request: Some(PullRequest {
                number: 10,
                title: "test PR".to_string(),
                labels: vec![],
                base: Some(PrBase {
                    r#ref: Some("main".to_string()),
                }),
            }),
            issue: None,
            comment: None,
            sender: Some(User {
                login: "testuser".to_string(),
            }),
            repository: Some(Repository {
                full_name: "owner/repo".to_string(),
            }),
        };
        let pr = wh.into_pr_event().unwrap();
        assert_eq!(pr.action, "opened");
        assert_eq!(pr.author, "testuser");
        assert_eq!(pr.owner, "owner");
        assert_eq!(pr.repo, "repo");
        assert_eq!(pr.pr_number, 10);
        assert_eq!(pr.title, "test PR");
        assert_eq!(pr.target_branch, "main");
    }

    #[test]
    fn webhook_event_into_pr_event_missing_pr() {
        let wh = WebhookEvent {
            action: "opened".to_string(),
            pull_request: None,
            issue: None,
            comment: None,
            sender: Some(User {
                login: "test".to_string(),
            }),
            repository: Some(Repository {
                full_name: "o/r".to_string(),
            }),
        };
        assert!(wh.into_pr_event().is_none());
    }

    #[test]
    fn webhook_event_into_pr_event_missing_sender() {
        let wh = WebhookEvent {
            action: "opened".to_string(),
            pull_request: Some(PullRequest {
                number: 1,
                title: "t".to_string(),
                labels: vec![],
                base: None,
            }),
            issue: None,
            comment: None,
            sender: None,
            repository: Some(Repository {
                full_name: "o/r".to_string(),
            }),
        };
        assert!(wh.into_pr_event().is_none());
    }

    #[test]
    fn webhook_event_into_comment_event() {
        let wh = WebhookEvent {
            action: "created".to_string(),
            pull_request: None,
            issue: Some(IssueRef {
                number: 5,
                pull_request: Some(serde_json::json!({})),
                body: None,
                labels: vec![],
            }),
            comment: Some(Comment {
                id: 1,
                body: "@janitor merge".to_string(),
            }),
            sender: Some(User {
                login: "dev".to_string(),
            }),
            repository: Some(Repository {
                full_name: "org/repo".to_string(),
            }),
        };
        let evt = wh.into_comment_event().unwrap();
        assert_eq!(evt.owner, "org");
        assert_eq!(evt.repo, "repo");
        assert_eq!(evt.pr_number, 5);
        assert_eq!(evt.author, "dev");
        assert_eq!(evt.body, "@janitor merge");
    }

    #[test]
    fn webhook_event_into_issue_comment_event() {
        let wh = WebhookEvent {
            action: "created".to_string(),
            pull_request: None,
            issue: Some(IssueRef {
                number: 7,
                pull_request: None,
                body: Some("issue body".to_string()),
                labels: vec![Label {
                    id: 1,
                    name: "bug".to_string(),
                }],
            }),
            comment: Some(Comment {
                id: 2,
                body: "@janitor ack".to_string(),
            }),
            sender: Some(User {
                login: "admin".to_string(),
            }),
            repository: Some(Repository {
                full_name: "org/repo".to_string(),
            }),
        };
        let evt = wh.into_issue_comment_event().unwrap();
        assert_eq!(evt.issue_number, 7);
        assert_eq!(evt.comment_body, "@janitor ack");
        assert_eq!(evt.issue_body, "issue body");
        assert_eq!(evt.issue_labels, vec!["bug"]);
    }

    #[test]
    fn webhook_into_pr_event_no_base_branch() {
        let wh = WebhookEvent {
            action: "opened".to_string(),
            pull_request: Some(PullRequest {
                number: 1,
                title: "t".to_string(),
                labels: vec![],
                base: None,
            }),
            issue: None,
            comment: None,
            sender: Some(User {
                login: "u".to_string(),
            }),
            repository: Some(Repository {
                full_name: "o/r".to_string(),
            }),
        };
        let pr = wh.into_pr_event().unwrap();
        assert_eq!(pr.target_branch, "");
    }
}
