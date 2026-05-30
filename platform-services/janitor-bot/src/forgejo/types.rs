use forgejo_api::structs::CommitStatusState;

#[derive(Debug, Clone)]
pub struct PrCombinedStatus {
    pub state: CommitStatusState,
    pub total_count: i64,
    pub statuses: Vec<PrStatusEntry>,
}

#[derive(Debug, Clone)]
pub struct PrStatusEntry {
    pub context: String,
    pub state: CommitStatusState,
}

pub struct BotComment {
    pub id: i64,
    pub body: String,
}

pub struct CommitStatusParams<'a> {
    pub owner: &'a str,
    pub repo: &'a str,
    pub sha: &'a str,
    pub state: &'a str,
    pub context: &'a str,
    pub description: &'a str,
    pub target_url: &'a str,
}

#[derive(serde::Deserialize)]
pub(super) struct RawCombinedStatus {
    #[serde(default)]
    pub state: Option<CommitStatusState>,
    #[serde(default)]
    pub total_count: Option<i64>,
    #[serde(default)]
    pub statuses: Vec<RawStatusEntry>,
}

#[derive(serde::Deserialize)]
pub(super) struct RawStatusEntry {
    #[serde(default)]
    pub context: Option<String>,
    #[serde(default)]
    pub status: Option<CommitStatusState>,
}
