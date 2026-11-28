mod gate;
mod userinfo;

use std::collections::BTreeSet;

use serde_json::Value;

pub use gate::{KanidmGate, groups_match};
use userinfo::UserInfoCache;

use super::{BoxFuture, Identity, IdentityKind, IssuerGate, IssuerStrategy};

pub struct Kanidm {
    name: String,
    base: String,
    userinfo: UserInfoCache,
}

impl Kanidm {
    pub fn new(name: String, base: String, client: reqwest::Client) -> Self {
        Self {
            name,
            base: base.trim_end_matches('/').to_owned(),
            userinfo: UserInfoCache::new(client),
        }
    }

    fn client<'i>(&self, iss: &'i str) -> Option<&'i str> {
        let client = iss
            .strip_prefix(self.base.as_str())?
            .strip_prefix('/')?
            .trim_end_matches('/');
        (!client.is_empty() && !client.contains('/')).then_some(client)
    }
}

pub fn token_scopes(claims: &Value) -> BTreeSet<String> {
    match claims.get("scope") {
        Some(Value::String(scope)) => scope.split_whitespace().map(str::to_owned).collect(),
        Some(Value::Array(scopes)) => scopes
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        _ => BTreeSet::new(),
    }
}

impl IssuerStrategy for Kanidm {
    fn name(&self) -> &str {
        &self.name
    }

    fn trusts(&self, iss: &str) -> bool {
        self.client(iss).is_some()
    }

    fn identify<'a>(&'a self, token: &'a str, claims: &'a Value) -> BoxFuture<'a, Identity> {
        Box::pin(async move {
            let text = |key: &str| claims.get(key).and_then(Value::as_str).map(str::to_owned);
            let iss = text("iss").unwrap_or_default();

            let mut groups = claims
                .get("groups")
                .and_then(Value::as_array)
                .map(|groups| {
                    groups
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect::<Vec<_>>()
                });
            let mut username = text("preferred_username");

            // Kanidm access tokens carry neither; userinfo has both
            if groups.is_none() || username.is_none() {
                let info = self.userinfo.get(&iss, token).await;
                groups.get_or_insert_with(|| info.groups.clone());
                username = username.or_else(|| info.preferred_username.clone());
            }

            Identity {
                issuer: self.name.clone(),
                kind: IdentityKind::User,
                client: self.client(&iss).map(str::to_owned),
                token_issuer: iss,
                subject: text("sub").unwrap_or_default(),
                username,
                scopes: token_scopes(claims),
                groups: groups.unwrap_or_default(),
                claims: Value::Null,
                expires_at: claims.get("exp").and_then(Value::as_i64),
            }
        })
    }

    fn gate(&self, section: Value) -> Result<Box<dyn IssuerGate>, String> {
        let gate: KanidmGate = serde_json::from_value(section)
            .map_err(|e| format!("expected clients and groups: {e}"))?;
        Ok(Box::new(gate))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn kanidm() -> Kanidm {
        Kanidm::new(
            "kanidm".to_owned(),
            "https://idm.example.test/oauth2/openid/".to_owned(),
            reqwest::Client::new(),
        )
    }

    #[test]
    fn trusts_one_client_level_under_the_base() {
        let kanidm = kanidm();
        assert!(kanidm.trusts("https://idm.example.test/oauth2/openid/graph"));
        assert_eq!(
            kanidm.client("https://idm.example.test/oauth2/openid/graph"),
            Some("graph")
        );
        assert!(!kanidm.trusts("https://idm.example.test/oauth2/openid"));
        assert!(!kanidm.trusts("https://idm.example.test/oauth2/openid/a/b"));
        assert!(!kanidm.trusts("https://idm.example.test/oauth2/openidx/graph"));
        assert!(!kanidm.trusts("https://git.example.test/api/actions"));
    }

    #[test]
    fn scopes_come_from_the_space_separated_claim() {
        let claims = json!({ "scope": "openid registry:package:read" });
        assert_eq!(
            token_scopes(&claims),
            BTreeSet::from(["openid".to_owned(), "registry:package:read".to_owned()])
        );
        assert_eq!(
            token_scopes(&json!({ "scope": ["a"] })),
            BTreeSet::from(["a".to_owned()])
        );
        assert!(token_scopes(&json!({})).is_empty());
    }

    #[tokio::test]
    async fn tokens_carrying_groups_and_username_skip_userinfo() {
        let claims = json!({
            "iss": "https://idm.example.test/oauth2/openid/graph",
            "sub": "uuid",
            "preferred_username": "anurag",
            "groups": ["spotify_users@idm.example.test"],
            "scope": "spotify:history:read",
            "exp": 42,
        });
        let identity = kanidm().identify("token", &claims).await;
        assert_eq!(identity.client.as_deref(), Some("graph"));
        assert_eq!(identity.username.as_deref(), Some("anurag"));
        assert_eq!(identity.groups, vec!["spotify_users@idm.example.test"]);
        assert!(identity.holds("spotify:history:read"));
        assert_eq!(identity.expires_at, Some(42));
    }
}
