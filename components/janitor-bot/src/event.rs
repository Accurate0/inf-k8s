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
pub struct PrEvent<'a> {
    pub action: &'a str,
    pub author: &'a str,
    pub owner: &'a str,
    pub repo: &'a str,
    pub pr_number: u64,
    pub title: &'a str,
    pub labels: &'a [Label],
}

impl WebhookEvent {
    pub fn as_pr_event(&self) -> Option<PrEvent<'_>> {
        let pr = self.pull_request.as_ref()?;
        let sender = self.sender.as_ref()?;
        let repository = self.repository.as_ref()?;
        let (owner, repo) = repository.full_name.split_once('/')?;
        Some(PrEvent {
            action: &self.action,
            author: &sender.login,
            owner,
            repo,
            pr_number: pr.number,
            title: &pr.title,
            labels: &pr.labels,
        })
    }
}
