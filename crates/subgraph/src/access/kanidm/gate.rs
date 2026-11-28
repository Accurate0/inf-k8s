use serde::Deserialize;

use crate::access::{Identity, IssuerGate};

#[derive(Debug, Clone, Deserialize)]
pub struct KanidmGate {
    clients: Vec<String>,

    #[serde(default)]
    groups: Vec<String>,
}

impl IssuerGate for KanidmGate {
    fn admits(&self, identity: &Identity, scoped: bool) -> bool {
        let client = identity
            .client
            .as_ref()
            .is_some_and(|client| self.clients.contains(client));
        client && (scoped || groups_match(&identity.groups, &self.groups))
    }
}

// kanidm sends groups as SPNs (name@domain) or uuids; either the SPN or its short name matches
pub fn groups_match(held: &[String], allowed: &[String]) -> bool {
    held.iter().any(|group| {
        let name = group
            .split_once('@')
            .map_or(group.as_str(), |(name, _)| name);
        allowed.iter().any(|a| a == group || a == name)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_match_by_short_name_or_spn() {
        let held = vec![
            "00000000-0000-0000-0000-000000000002".to_owned(),
            "spotify_users@idm.example.test".to_owned(),
        ];
        assert!(groups_match(&held, &["spotify_users".to_owned()]));
        assert!(groups_match(
            &held,
            &["spotify_users@idm.example.test".to_owned()]
        ));
        assert!(!groups_match(&held, &["platform_admins".to_owned()]));
        assert!(!groups_match(&[], &["spotify_users".to_owned()]));
    }
}
