mod introspection;

use std::collections::{BTreeSet, HashSet};
use std::sync::Arc;
use std::time::Duration;

use hive_router::{
    ArcSwap, GraphQLError, async_trait,
    executor::execution::client_request_details::JwtRequestDetails,
    http::{
        StatusCode,
        header::{HeaderName, HeaderValue},
    },
    ntex::http::{Method, Response, header::CONTENT_TYPE},
    plugins::{
        hooks::{
            on_execute::{OnExecuteStartHookPayload, OnExecuteStartHookResult},
            on_graphql_analysis::{OnGraphqlAnalysisHookPayload, OnGraphqlAnalysisHookResult},
            on_http_request::{OnHttpRequestHookFuture, OnHttpRequestHookPayload},
            on_plugin_init::{OnPluginInitPayload, OnPluginInitResult},
            on_query_plan::{OnQueryPlanStartHookPayload, OnQueryPlanStartHookResult},
            on_subgraph_execute::{
                OnSubgraphExecuteStartHookPayload, OnSubgraphExecuteStartHookResult,
            },
            on_supergraph_load::{
                OnSupergraphLoadStartHookPayload, OnSupergraphLoadStartHookResult,
            },
        },
        plugin_trait::{EndHookPayload, RouterPlugin, StartHookPayload},
    },
    query_planner::planner::plan_nodes::QueryPlan,
    tracing,
};
use serde::Deserialize;
use subgraph::access::{Identity, IdentityKind, Policy};
use subgraph::passport::{self, Minter};

use introspection::{Hidden, Ownership};

#[derive(Deserialize)]
pub struct AccessConfig {
    #[serde(flatten)]
    policy: subgraph::access::AccessConfig,
    passport: PassportConfig,
}

#[derive(Deserialize)]
struct PassportConfig {
    seed_env: String,
    ttl_secs: u64,
}

pub struct AccessPlugin {
    policy: Policy,
    minter: Minter,
    ownership: ArcSwap<Ownership>,
}

#[derive(Clone)]
pub struct CallerSummary {
    pub kind: CallerKind,
    pub subject: String,
    pub issuer: String,
    pub client: Option<String>,
    pub username: Option<String>,
    pub expires_at: Option<i64>,
    pub scopes: BTreeSet<String>,
    pub subgraphs: Vec<String>,
    pub token: String,
}

#[derive(Clone, PartialEq, Eq)]
pub enum CallerKind {
    User,
    External(String),
}

#[derive(Clone)]
struct Caller(Arc<Identity>);

#[derive(Clone, Default)]
struct Denied(Arc<HashSet<String>>);

#[async_trait]
impl RouterPlugin for AccessPlugin {
    type Config = AccessConfig;

    fn plugin_name() -> &'static str {
        "access"
    }

    fn on_plugin_init(payload: OnPluginInitPayload<Self>) -> OnPluginInitResult<Self> {
        let config = payload.config()?;

        let seed_env = &config.passport.seed_env;
        let seed = std::env::var(seed_env).map_err(|_| format!("{seed_env} is not set"))?;
        let minter = Minter::from_seed(&seed, Duration::from_secs(config.passport.ttl_secs))?;

        payload.initialize_plugin(Self {
            policy: Policy::new(config.policy)?,
            minter,
            ownership: ArcSwap::default(),
        })
    }

    fn on_http_request<'req>(
        &'req self,
        payload: OnHttpRequestHookPayload<'req>,
    ) -> OnHttpRequestHookFuture<'req> {
        Box::pin(async move {
            let request = &payload.router_http_request;
            if request.path() != passport::JWKS_PATH || request.method() != Method::GET {
                return payload.proceed();
            }

            let jwks = self.minter.jwks().to_owned();
            payload.end_with_response(
                Response::Ok()
                    .header(CONTENT_TYPE, "application/json")
                    .body(jwks),
            )
        })
    }

    fn on_supergraph_reload<'exec>(
        &'exec self,
        payload: OnSupergraphLoadStartHookPayload,
    ) -> OnSupergraphLoadStartHookResult<'exec> {
        let ownership = Arc::new(Ownership::from(&payload.new_ast));
        payload.on_end(move |end| {
            self.ownership.store(ownership);
            end.proceed()
        })
    }

    async fn on_graphql_analysis<'exec>(
        &'exec self,
        payload: &mut OnGraphqlAnalysisHookPayload<'exec>,
    ) -> OnGraphqlAnalysisHookResult {
        let identity = match payload.client_request_details.jwt.as_ref() {
            JwtRequestDetails::Authenticated { token, claims, .. } => {
                let claims: serde_json::Value = hive_router::sonic_rs::to_string(claims)
                    .ok()
                    .and_then(|json| serde_json::from_str(&json).ok())
                    .unwrap_or_default();
                self.policy.resolve(token, &claims).await.map(Arc::new)
            }
            JwtRequestDetails::Unauthenticated => None,
        };

        let policies: Vec<String> = payload
            .request_context
            .read()
            .ok()
            .and_then(|context| {
                context
                    .authorization()
                    .required_policies()
                    .map(|policies| policies.keys().cloned().collect())
            })
            .unwrap_or_default();
        if !policies.is_empty()
            && let Ok(mut context) = payload.request_context.write()
        {
            let mut authorization = context.authorization();
            for policy in policies {
                let granted = identity.as_ref().is_some_and(|id| id.holds(&policy));
                authorization.set_policy_decision(policy, granted);
            }
        }

        let reachable = self.policy.reachable(identity.as_deref());
        let denied: HashSet<String> = self
            .policy
            .subgraphs()
            .filter(|subgraph| !reachable.contains(*subgraph))
            .map(str::to_owned)
            .collect();

        if let (Some(identity), JwtRequestDetails::Authenticated { token, .. }) =
            (&identity, payload.client_request_details.jwt.as_ref())
        {
            payload.context.insert(summary(identity, token, &reachable));
            payload.context.insert(Caller(identity.clone()));
        }
        payload.context.insert(Denied(Arc::new(denied)));

        OnGraphqlAnalysisHookResult::Proceed
    }

    async fn on_query_plan<'exec>(
        &'exec self,
        payload: OnQueryPlanStartHookPayload<'exec>,
    ) -> OnQueryPlanStartHookResult<'exec> {
        let denied = self.denied(payload.context.get_ref::<Denied>().map(|d| d.clone()));
        if denied.0.is_empty() {
            return payload.proceed();
        }

        payload.on_end(move |end| {
            match denied_subscription(end.query_plan(), &denied.0).map(str::to_owned) {
                Some(subgraph) => end.end_with_graphql_error(
                    GraphQLError::from_message_and_code(
                        format!("not allowed to query the {subgraph} subgraph"),
                        "FORBIDDEN",
                    ),
                    StatusCode::FORBIDDEN,
                ),
                None => end.proceed(),
            }
        })
    }

    async fn on_subgraph_execute<'exec>(
        &'exec self,
        mut payload: OnSubgraphExecuteStartHookPayload<'exec>,
    ) -> OnSubgraphExecuteStartHookResult<'exec> {
        let denied = self.denied(payload.context.get_ref::<Denied>().map(|d| d.clone()));
        if denied.0.contains(payload.subgraph_name) {
            let message = format!(
                "not allowed to query the {} subgraph",
                payload.subgraph_name
            );
            return payload.end_with_graphql_error(
                GraphQLError::from_message_and_code(message, "FORBIDDEN"),
                StatusCode::FORBIDDEN,
            );
        }

        let Some(caller) = payload.context.get_ref::<Caller>().map(|c| c.0.clone()) else {
            return payload.proceed();
        };

        let header = self
            .minter
            .mint(&caller, payload.subgraph_name)
            .and_then(|token| HeaderValue::from_str(&token).map_err(|e| e.to_string()));
        match header {
            Ok(value) => {
                payload
                    .execution_request
                    .headers
                    .insert(HeaderName::from_static(passport::HEADER), value);
                payload.proceed()
            }
            Err(error) => {
                tracing::error!(
                    subgraph = payload.subgraph_name,
                    "passport signing failed: {error}"
                );
                payload.end_with_graphql_error(
                    GraphQLError::from_message_and_code(
                        "could not sign the request for the subgraph",
                        "INTERNAL_SERVER_ERROR",
                    ),
                    StatusCode::INTERNAL_SERVER_ERROR,
                )
            }
        }
    }

    async fn on_execute<'exec>(
        &'exec self,
        mut payload: OnExecuteStartHookPayload<'exec>,
    ) -> OnExecuteStartHookResult<'exec> {
        let denied = self.denied(payload.context.get_ref::<Denied>().map(|d| d.clone()));
        if !denied.0.is_empty() {
            let hidden = Hidden {
                ownership: &self.ownership.load(),
                denied: &denied.0,
            };
            hidden.prune_response(&mut payload.data);
        }

        payload.proceed()
    }
}

impl AccessPlugin {
    // no context means analysis never ran for this request, so nothing gated is allowed
    fn denied(&self, context: Option<Denied>) -> Denied {
        context.unwrap_or_else(|| {
            Denied(Arc::new(
                self.policy.subgraphs().map(str::to_owned).collect(),
            ))
        })
    }
}

fn summary(identity: &Identity, token: &str, reachable: &BTreeSet<String>) -> CallerSummary {
    CallerSummary {
        kind: match identity.kind {
            IdentityKind::User => CallerKind::User,
            IdentityKind::External => CallerKind::External(identity.issuer.clone()),
        },
        subject: identity.subject.clone(),
        issuer: identity.token_issuer.clone(),
        client: identity.client.clone(),
        username: identity.username.clone(),
        expires_at: identity.expires_at,
        scopes: identity.scopes.clone(),
        subgraphs: reachable.iter().cloned().collect(),
        token: token.to_owned(),
    }
}

// the subgraph execute hook can't fail a subscription's registration, so it's rejected once planned
fn denied_subscription<'p>(plan: &'p QueryPlan, denied: &HashSet<String>) -> Option<&'p str> {
    let (subscription, _) = plan.split_subscription()?;
    let name = &subscription.primary.service_name;
    denied.contains(name).then_some(name.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn generated() -> AccessConfig {
        let router: serde_yaml::Value =
            serde_yaml::from_str(include_str!("../../supergraph/router.yaml")).unwrap();
        serde_yaml::from_value(router["plugins"]["access"]["config"].clone()).unwrap()
    }

    #[tokio::test]
    async fn the_generated_router_config_gates_real_callers() {
        let config = generated();
        assert_eq!(config.passport.seed_env, "GRAPH_PASSPORT_SEED");
        let policy = Policy::new(config.policy).unwrap();

        let forgejo = policy
            .resolve(
                "token",
                &json!({
                    "iss": "https://git.anurag.sh/api/actions",
                    "sub": "repo:anurag/k8s:ref:refs/heads/main",
                    "repository": "anurag/k8s",
                }),
            )
            .await
            .unwrap();
        assert!(forgejo.holds("registry:package:publish"));
        assert!(policy.admits("registry", &forgejo));
        assert!(!policy.admits("spotify", &forgejo));

        let user = policy
            .resolve(
                "token",
                &json!({
                    "iss": "https://idm.anurag.sh/oauth2/openid/spotify",
                    "sub": "uuid",
                    "preferred_username": "anurag",
                    "groups": [],
                    "scope": "openid spotify:history:read",
                }),
            )
            .await
            .unwrap();
        assert!(policy.admits("spotify", &user));
        assert!(!policy.admits("bom", &user), "bom only takes graph tokens");
    }
}
