use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A resolved virtual key, as seen on the request hot path.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VirtualKey {
    pub id: Uuid,
    pub name: String,
    pub allowed_models: Vec<String>,
    pub monthly_token_budget: Option<i64>,
}

impl VirtualKey {
    /// Empty `allowed_models` means "any model".
    pub fn allows(&self, model: &str) -> bool {
        self.allowed_models.is_empty() || self.allowed_models.iter().any(|m| m == model)
    }
}

#[derive(Clone, Serialize)]
pub struct KeyInfo {
    pub id: Uuid,
    pub name: String,
    pub allowed_models: Vec<String>,
    pub monthly_token_budget: Option<i64>,
    pub revoked: bool,
    pub created_at: DateTime<Utc>,
}

/// Partial update payload; absent fields are left unchanged. `monthly_token_budget`
/// can only be set, not cleared back to null, through this path.
#[derive(Debug, Default, Deserialize)]
pub struct UpdateKey {
    pub name: Option<String>,
    pub allowed_models: Option<Vec<String>>,
    pub monthly_token_budget: Option<i64>,
    pub revoked: Option<bool>,
}

#[derive(sqlx::FromRow)]
pub(super) struct KeyRow {
    pub id: Uuid,
    pub name: String,
    pub allowed_models: Vec<String>,
    pub monthly_token_budget: Option<i64>,
    pub revoked: bool,
    pub created_at: DateTime<Utc>,
}

impl From<KeyRow> for KeyInfo {
    fn from(r: KeyRow) -> Self {
        KeyInfo {
            id: r.id,
            name: r.name,
            allowed_models: r.allowed_models,
            monthly_token_budget: r.monthly_token_budget,
            revoked: r.revoked,
            created_at: r.created_at,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_allowlist_permits_any_model() {
        let key = VirtualKey {
            id: Uuid::nil(),
            name: "t".into(),
            allowed_models: vec![],
            monthly_token_budget: None,
        };
        assert!(key.allows("anything"));
    }

    #[test]
    fn allowlist_restricts_models() {
        let key = VirtualKey {
            id: Uuid::nil(),
            name: "t".into(),
            allowed_models: vec!["claude-fable-5".into()],
            monthly_token_budget: None,
        };
        assert!(key.allows("claude-fable-5"));
        assert!(!key.allows("gpt-4o"));
    }
}
