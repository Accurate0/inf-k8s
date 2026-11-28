mod claims;
mod gate;

use std::collections::BTreeSet;

use serde_json::Value;

pub use claims::{ClaimPattern, OneOrMany, any_matches};
pub use gate::ExternalGate;

use super::{BoxFuture, Grants, Identity, IdentityKind, IssuerGate, IssuerStrategy};

pub struct External {
    name: String,
    issuer: String,
    grants: Vec<(String, ClaimPattern)>,
}

impl External {
    pub fn new(name: String, issuer: String, grants: Vec<(String, ClaimPattern)>) -> Self {
        Self {
            name,
            issuer,
            grants,
        }
    }
}

pub fn grants_for(grants: &Grants, issuer: &str) -> Vec<(String, ClaimPattern)> {
    grants
        .iter()
        .flat_map(|(scope, issuers)| {
            issuers
                .get(issuer)
                .into_iter()
                .flatten()
                .map(move |pattern| (scope.clone(), pattern.clone()))
        })
        .collect()
}

pub fn granted(grants: &Grants, issuer: &str, claims: &Value) -> BTreeSet<String> {
    grants_for(grants, issuer)
        .into_iter()
        .filter(|(_, pattern)| pattern.matches(claims))
        .map(|(scope, _)| scope)
        .collect()
}

impl IssuerStrategy for External {
    fn name(&self) -> &str {
        &self.name
    }

    fn trusts(&self, iss: &str) -> bool {
        iss == self.issuer
    }

    fn identify<'a>(&'a self, _token: &'a str, claims: &'a Value) -> BoxFuture<'a, Identity> {
        let text = |key: &str| claims.get(key).and_then(Value::as_str).map(str::to_owned);
        let identity = Identity {
            issuer: self.name.clone(),
            kind: IdentityKind::External,
            token_issuer: self.issuer.clone(),
            subject: text("sub").unwrap_or_default(),
            username: None,
            client: text("aud"),
            scopes: self
                .grants
                .iter()
                .filter(|(_, pattern)| pattern.matches(claims))
                .map(|(scope, _)| scope.clone())
                .collect(),
            groups: Vec::new(),
            claims: claims.clone(),
            expires_at: claims.get("exp").and_then(Value::as_i64),
        };

        Box::pin(async move { identity })
    }

    fn gate(&self, section: Value) -> Result<Box<dyn IssuerGate>, String> {
        let patterns: Vec<ClaimPattern> =
            serde_json::from_value(section).map_err(|e| format!("expected claim patterns: {e}"))?;
        Ok(Box::new(ExternalGate::new(patterns)))
    }
}
