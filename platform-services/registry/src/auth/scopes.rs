//! The registry's OAuth scopes, declared with their granting groups in the registry subgraph's
//! auth.scopes in app.yaml and issued by Kanidm.

use std::collections::BTreeSet;

pub const PACKAGE_READ: &str = "registry:package:read";
pub const IMAGE_READ: &str = "registry:image:read";
pub const PACKAGE_WRITE: &str = "registry:package:write";
pub const PACKAGE_PUBLISH: &str = "registry:package:publish";
pub const PUBLISHER_MANAGE: &str = "registry:publisher:manage";
pub const TOKEN_MANAGE: &str = "registry:token:manage";
pub const AUDIT_READ: &str = "registry:audit:read";

const ALL: [&str; 7] = [
    PACKAGE_READ,
    IMAGE_READ,
    PACKAGE_WRITE,
    PACKAGE_PUBLISH,
    PUBLISHER_MANAGE,
    TOKEN_MANAGE,
    AUDIT_READ,
];

/// Publish tokens are the registry's own credential; trusted publishing and their package scopes
/// still decide what they may publish.
pub const TOKEN: [&str; 3] = [PACKAGE_READ, IMAGE_READ, PACKAGE_PUBLISH];

pub fn owned(scopes: &[&str]) -> BTreeSet<String> {
    scopes.iter().map(|scope| (*scope).to_owned()).collect()
}

/// The space-separated `scope` claim, keeping only the registry's own scopes.
pub fn from_claim(claim: Option<&str>) -> BTreeSet<String> {
    claim
        .unwrap_or_default()
        .split_whitespace()
        .filter(|scope| scope.starts_with("registry:"))
        .map(str::to_owned)
        .collect()
}

/// Until Kanidm issues the registry's scopes, a token without any falls back to the groups that
/// granted the same access before.
pub fn from_groups(admin: bool, reader: bool) -> BTreeSet<String> {
    if admin {
        owned(&ALL)
    } else if reader {
        owned(&[PACKAGE_READ, IMAGE_READ])
    } else {
        BTreeSet::new()
    }
}
