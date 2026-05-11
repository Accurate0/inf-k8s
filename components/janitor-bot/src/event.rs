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
    pub sender: Option<User>,
    pub repository: Option<Repository>,
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
pub struct WorkflowEvent {
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
}

pub enum BotEvent<'a> {
    ForgejoPr(&'a PrEvent),
    GitHubWorkflow(&'a WorkflowEvent),
}

impl BotEvent<'_> {
    pub fn template_vars(&self) -> std::collections::HashMap<&'static str, String> {
        let mut vars = std::collections::HashMap::new();
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
            BotEvent::GitHubWorkflow(wf) => {
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

pub fn render_template(
    template: &str,
    vars: &std::collections::HashMap<&str, String>,
) -> String {
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
