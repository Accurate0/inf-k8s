use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use wildmatch::WildMatch;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(untagged)]
pub enum OneOrMany {
    One(String),
    Many(Vec<String>),
}

impl OneOrMany {
    fn globs(&self) -> &[String] {
        match self {
            Self::One(glob) => std::slice::from_ref(glob),
            Self::Many(globs) => globs,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(transparent)]
pub struct ClaimPattern(pub BTreeMap<String, OneOrMany>);

impl ClaimPattern {
    pub fn matches(&self, claims: &Value) -> bool {
        self.0.iter().all(|(name, allowed)| {
            let matches = |value: &str| {
                let value = value.to_lowercase();
                allowed
                    .globs()
                    .iter()
                    .any(|glob| WildMatch::new(&glob.to_lowercase()).matches(&value))
            };

            match claims.get(name) {
                Some(Value::String(value)) => matches(value),
                Some(Value::Array(values)) => values.iter().filter_map(Value::as_str).any(matches),
                _ => false,
            }
        })
    }
}

pub fn any_matches(patterns: &[ClaimPattern], claims: &Value) -> bool {
    patterns.iter().any(|pattern| pattern.matches(claims))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn pattern(claims: Value) -> ClaimPattern {
        serde_json::from_value(claims).unwrap()
    }

    #[test]
    fn every_listed_claim_must_match_one_glob() {
        let claims = json!({
            "repository": "Anurag/k8s",
            "ref": "refs/heads/main",
            "event_name": "push",
        });

        assert!(pattern(json!({ "repository": "*" })).matches(&claims));
        assert!(
            pattern(json!({ "repository": "anurag/k8s", "ref": "refs/heads/main" }))
                .matches(&claims)
        );
        assert!(pattern(json!({ "event_name": ["push", "workflow_dispatch"] })).matches(&claims));
        assert!(!pattern(json!({ "repository": "anurag/spotify" })).matches(&claims));
        assert!(!pattern(json!({ "workflow_ref": "*" })).matches(&claims));
    }

    #[test]
    fn a_list_claim_matches_when_any_string_in_it_does() {
        let admin = json!({ "roles": ["points", "Admin"] });
        let user = json!({ "roles": ["points"] });
        let none = json!({ "roles": [] });
        let odd = json!({ "roles": [1, true, null, { "name": "admin" }] });
        let is_admin = pattern(json!({ "roles": "admin" }));

        assert!(is_admin.matches(&admin));
        assert!(!is_admin.matches(&user));
        assert!(!is_admin.matches(&none));
        assert!(!is_admin.matches(&odd));
        assert!(pattern(json!({ "roles": "*" })).matches(&user));
        assert!(!pattern(json!({ "roles": "*" })).matches(&none));
        assert!(pattern(json!({ "roles": ["admin", "points"] })).matches(&user));
        assert!(!is_admin.matches(&json!({ "roles": 1 })));
    }

    #[test]
    fn pull_requests_can_be_kept_out() {
        let pr = json!({ "repository": "anurag/k8s", "event_name": "pull_request" });
        let only_pushes =
            pattern(json!({ "event_name": ["push", "workflow_dispatch", "schedule"] }));
        assert!(!only_pushes.matches(&pr));
    }
}
