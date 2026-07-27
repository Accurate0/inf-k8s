use forgejo_api::structs::CommitStatusState;
use serde::Deserialize;

/// Forgejo returns `"state": ""` for commits with no CI statuses, and could add
/// states our `forgejo_api` version doesn't know. Deserialize leniently so an
/// empty or unknown value maps to `None` instead of failing the whole decode.
fn lenient_status<'de, D>(d: D) -> Result<Option<CommitStatusState>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use CommitStatusState::*;

    Ok(Option::<String>::deserialize(d)?.and_then(|s| match s.as_str() {
        "pending" => Some(Pending),
        "success" => Some(Success),
        "error" => Some(Error),
        "failure" => Some(Failure),
        "warning" => Some(Warning),
        "skipped" => Some(Skipped),
        _ => None,
    }))
}

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
    pub description: String,
    pub target_url: String,
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
    #[serde(default, deserialize_with = "lenient_status")]
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
    #[serde(default, deserialize_with = "lenient_status")]
    pub status: Option<CommitStatusState>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub target_url: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_state_decodes_to_none() {
        let raw: RawCombinedStatus =
            serde_json::from_str(r#"{"state":"","total_count":0,"statuses":[]}"#).unwrap();

        assert_eq!(raw.state, None);
        assert_eq!(raw.total_count, Some(0));
        assert!(raw.statuses.is_empty());
    }

    #[test]
    fn unknown_state_decodes_to_none() {
        let raw: RawCombinedStatus =
            serde_json::from_str(r#"{"state":"queued"}"#).unwrap();

        assert_eq!(raw.state, None);
    }

    #[test]
    fn known_state_decodes() {
        let raw: RawCombinedStatus =
            serde_json::from_str(r#"{"state":"success"}"#).unwrap();

        assert_eq!(raw.state, Some(CommitStatusState::Success));
    }
}
