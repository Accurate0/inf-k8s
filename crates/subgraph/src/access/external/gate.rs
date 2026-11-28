use super::ClaimPattern;
use crate::access::{Identity, IssuerGate};

pub struct ExternalGate {
    patterns: Vec<ClaimPattern>,
}

impl ExternalGate {
    pub fn new(patterns: Vec<ClaimPattern>) -> Self {
        Self { patterns }
    }
}

impl IssuerGate for ExternalGate {
    fn admits(&self, identity: &Identity, scoped: bool) -> bool {
        scoped || super::any_matches(&self.patterns, &identity.claims)
    }
}
