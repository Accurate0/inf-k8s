use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct PullRequest {
    pub number: u64,
    pub title: String,
    #[serde(default)]
    pub labels: Vec<Label>,
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
}

pub enum BotEvent<'a> {
    ForgejoPr(&'a PrEvent),
    GitHubWorkflow(&'a WorkflowEvent),
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
            labels: pr.labels,
            changed_files: Vec::new(),
        })
    }
}
