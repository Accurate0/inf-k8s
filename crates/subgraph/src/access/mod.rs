mod config;
pub mod external;
pub mod kanidm;

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use config::{AccessConfig, Grants, IssuerConfig, SubgraphGate};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum IdentityKind {
    User,
    External,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct Identity {
    pub issuer: String,
    pub kind: IdentityKind,
    pub token_issuer: String,
    pub subject: String,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub client: Option<String>,
    #[serde(default)]
    pub scopes: BTreeSet<String>,
    #[serde(default)]
    pub groups: Vec<String>,
    #[serde(default)]
    pub claims: Value,
    #[serde(default)]
    pub expires_at: Option<i64>,
}

impl Identity {
    pub fn holds(&self, scope: &str) -> bool {
        self.scopes.contains(scope)
    }
}

pub trait IssuerStrategy: Send + Sync {
    fn name(&self) -> &str;

    fn trusts(&self, iss: &str) -> bool;

    fn identify<'a>(&'a self, token: &'a str, claims: &'a Value) -> BoxFuture<'a, Identity>;

    fn gate(&self, section: Value) -> Result<Box<dyn IssuerGate>, String>;
}

pub trait IssuerGate: Send + Sync {
    fn admits(&self, identity: &Identity, scoped: bool) -> bool;
}

struct Gate {
    scopes: Vec<String>,
    issuers: BTreeMap<String, Box<dyn IssuerGate>>,
}

pub struct Policy {
    strategies: Vec<Arc<dyn IssuerStrategy>>,
    subgraphs: BTreeMap<String, Gate>,
}

impl Policy {
    pub fn new(config: AccessConfig) -> Result<Self, String> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .map_err(|e| e.to_string())?;
        let strategies: BTreeMap<String, Arc<dyn IssuerStrategy>> = config
            .issuers
            .into_iter()
            .map(|(name, issuer)| {
                let strategy: Arc<dyn IssuerStrategy> = match issuer {
                    IssuerConfig::Kanidm { base } => {
                        Arc::new(kanidm::Kanidm::new(name.clone(), base, client.clone()))
                    }
                    IssuerConfig::External { issuer } => Arc::new(external::External::new(
                        name.clone(),
                        issuer,
                        external::grants_for(&config.scopes, &name),
                    )),
                };

                (name, strategy)
            })
            .collect();

        let mut subgraphs = BTreeMap::new();
        for (subgraph, gate) in config.subgraphs {
            let mut issuers = BTreeMap::new();
            for (name, section) in gate.issuers {
                let strategy = strategies.get(&name).ok_or_else(|| {
                    format!("subgraphs.{subgraph}.{name}: no issuer named {name}")
                })?;
                let issuer_gate = strategy
                    .gate(section)
                    .map_err(|e| format!("subgraphs.{subgraph}.{name}: {e}"))?;
                issuers.insert(name, issuer_gate);
            }

            subgraphs.insert(
                subgraph,
                Gate {
                    scopes: gate.scopes,
                    issuers,
                },
            );
        }

        Ok(Self {
            strategies: strategies.into_values().collect(),
            subgraphs,
        })
    }

    pub async fn resolve(&self, token: &str, claims: &Value) -> Option<Identity> {
        let iss = claims.get("iss")?.as_str()?;
        let strategy = self.strategies.iter().find(|s| s.trusts(iss))?;
        Some(strategy.identify(token, claims).await)
    }

    pub fn is_gated(&self) -> bool {
        !self.subgraphs.is_empty()
    }

    pub fn subgraphs(&self) -> impl Iterator<Item = &str> {
        self.subgraphs.keys().map(String::as_str)
    }

    pub fn admits(&self, subgraph: &str, identity: &Identity) -> bool {
        let Some(gate) = self.subgraphs.get(subgraph) else {
            return true;
        };
        let scoped = gate.scopes.iter().any(|scope| identity.holds(scope));
        gate.issuers
            .get(&identity.issuer)
            .is_some_and(|issuer| issuer.admits(identity, scoped))
    }

    pub fn reachable(&self, identity: Option<&Identity>) -> BTreeSet<String> {
        let Some(identity) = identity else {
            return BTreeSet::new();
        };
        self.subgraphs
            .keys()
            .filter(|subgraph| self.admits(subgraph, identity))
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const KANIDM: &str = "https://idm.example.test/oauth2/openid";
    const FORGEJO: &str = "https://git.example.test/api/actions";

    fn policy() -> Policy {
        let config: AccessConfig = serde_json::from_value(json!({
            "issuers": {
                "kanidm": { "kind": "kanidm", "base": KANIDM },
                "forgejo": { "kind": "external", "issuer": FORGEJO },
            },
            "scopes": {
                "registry:package:publish": {
                    "forgejo": [{ "repository": "anurag/*", "ref": "refs/heads/main" }],
                },
            },
            "subgraphs": {
                "registry": {
                    "scopes": ["registry:package:read", "registry:package:publish"],
                    "kanidm": { "clients": ["graph", "registry"], "groups": ["registry_users"] },
                    "forgejo": [],
                },
                "spotify": {
                    "kanidm": { "clients": ["spotify"], "groups": ["spotify_users"] },
                },
                "secret": {
                    "forgejo": [{ "repository": "anurag/k8s" }],
                },
            },
        }))
        .unwrap();
        Policy::new(config).unwrap()
    }

    fn user(client: &str, scopes: &[&str], groups: &[&str]) -> Identity {
        Identity {
            issuer: "kanidm".to_owned(),
            kind: IdentityKind::User,
            token_issuer: format!("{KANIDM}/{client}"),
            subject: "user".to_owned(),
            username: Some("user".to_owned()),
            client: Some(client.to_owned()),
            scopes: scopes.iter().map(|s| (*s).to_owned()).collect(),
            groups: groups.iter().map(|g| (*g).to_owned()).collect(),
            claims: Value::Null,
            expires_at: None,
        }
    }

    async fn forgejo(policy: &Policy, claims: Value) -> Identity {
        let mut claims = claims;
        claims["iss"] = json!(FORGEJO);
        claims["sub"] = json!("repo:anurag/k8s");
        policy.resolve("token", &claims).await.unwrap()
    }

    #[test]
    fn users_need_one_of_the_gates_clients_and_a_scope_or_group() {
        let policy = policy();
        assert!(policy.admits("registry", &user("graph", &["registry:package:read"], &[])));
        assert!(policy.admits(
            "registry",
            &user("registry", &[], &["registry_users@idm.example.test"])
        ));
        assert!(!policy.admits(
            "registry",
            &user("spotify", &["registry:package:read"], &[])
        ));
        assert!(!policy.admits(
            "registry",
            &user("graph", &["waf:block:read"], &["waf_admins"])
        ));
        assert!(!policy.admits("secret", &user("graph", &[], &[])));
    }

    #[test]
    fn ungated_subgraphs_admit_everyone() {
        assert!(policy().admits("node", &user("graph", &[], &[])));
    }

    #[tokio::test]
    async fn external_identities_get_scopes_from_grants_and_pass_by_scope_or_pattern() {
        let policy = policy();

        let main = forgejo(
            &policy,
            json!({ "repository": "anurag/k8s", "ref": "refs/heads/main" }),
        )
        .await;
        assert_eq!(main.issuer, "forgejo");
        assert_eq!(main.kind, IdentityKind::External);
        assert_eq!(
            main.scopes,
            BTreeSet::from(["registry:package:publish".to_owned()])
        );
        assert!(policy.admits("registry", &main));
        assert!(policy.admits("secret", &main));
        assert!(!policy.admits("spotify", &main));

        let feature = forgejo(
            &policy,
            json!({ "repository": "anurag/k8s", "ref": "refs/heads/x" }),
        )
        .await;
        assert!(feature.scopes.is_empty());
        assert!(!policy.admits("registry", &feature));
        assert!(policy.admits("secret", &feature));

        let other = forgejo(
            &policy,
            json!({ "repository": "someone/else", "ref": "refs/heads/main" }),
        )
        .await;
        assert!(!policy.admits("registry", &other));
        assert!(!policy.admits("secret", &other));
    }

    #[tokio::test]
    async fn unknown_issuers_resolve_to_nobody() {
        let claims = json!({ "iss": "https://elsewhere.test", "sub": "x" });
        assert!(policy().resolve("token", &claims).await.is_none());
    }

    #[test]
    fn reachable_lists_the_admitting_subgraphs() {
        let policy = policy();
        let reachable = policy.reachable(Some(&user("graph", &["registry:package:read"], &[])));
        assert_eq!(reachable, BTreeSet::from(["registry".to_owned()]));
        assert!(policy.reachable(None).is_empty());
    }

    #[test]
    fn gates_naming_an_unknown_issuer_are_rejected() {
        let config: AccessConfig = serde_json::from_value(json!({
            "issuers": {},
            "subgraphs": { "x": { "github": [] } },
        }))
        .unwrap();
        let error = Policy::new(config).err().unwrap();
        assert_eq!(error, "subgraphs.x.github: no issuer named github");
    }
}
