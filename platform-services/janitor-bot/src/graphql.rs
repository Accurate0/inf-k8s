use std::sync::Arc;
use std::time::Duration;

use async_graphql::futures_util::Stream;
use async_graphql::{
    ComplexObject, Context, Enum, Error, ErrorExtensions, ID, InputObject, Interface, Json, Object,
    Result, Schema, SchemaBuilder, SimpleObject, Subscription,
};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use serde_json::Value;
use subgraph::node::Ids;
use subgraph::{AuthConfig, Authenticator};

use crate::AppState;
use crate::routes;

const IDS: Ids = Ids::new("janitor");

pub fn builder() -> SchemaBuilder<QueryRoot, MutationRoot, SubscriptionRoot> {
    Schema::build(QueryRoot, MutationRoot, SubscriptionRoot)
        .register_output_type::<Node>()
        .enable_federation()
        .enable_subscription_in_federation()
}

pub fn router(state: Arc<AppState>, auth: AuthConfig) -> axum::Router {
    let auth = Authenticator::new(auth);
    tokio::spawn(auth.clone().run());

    subgraph::router(builder().data(state).finish(), auth)
}

fn state(ctx: &Context<'_>) -> Arc<AppState> {
    ctx.data_unchecked::<Arc<AppState>>().clone()
}

/// The admin handlers answer with a status and a JSON body; anything but 2xx is an error.
fn outcome((status, body): (StatusCode, axum::Json<Value>)) -> Result<Json<Value>> {
    if status.is_success() {
        return Ok(Json(body.0));
    }
    let message = body
        .0
        .get("error")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| status.to_string());
    let code = if status.is_client_error() {
        "BAD_USER_INPUT"
    } else {
        "UPSTREAM"
    };
    Err(Error::new(message).extend_with(|_, ext| ext.set("code", code)))
}

#[derive(Interface)]
#[graphql(field(name = "id", ty = "ID"))]
pub enum Node {
    Rule(Rule),
}

#[derive(SimpleObject)]
#[graphql(name = "JanitorRule", complex)]
pub struct Rule {
    pub name: String,
    /// `true`, `false` or `dry_run`.
    pub enabled: String,
    pub priority: i32,
    pub depends_on: Vec<String>,
}

#[ComplexObject]
impl Rule {
    async fn id(&self) -> ID {
        IDS.encode("rule", &self.name)
    }
}

fn rules(ctx: &Context<'_>) -> Vec<Rule> {
    state(ctx)
        .orchestrator
        .rules_summary()
        .into_iter()
        .map(|r| Rule {
            name: r.name,
            enabled: r.enabled,
            priority: r.priority,
            depends_on: r.depends_on,
        })
        .collect()
}

#[derive(SimpleObject, Clone)]
#[graphql(name = "JanitorRuleAction")]
pub struct RuleAction {
    pub action: String,
    pub ran: bool,
}

#[derive(SimpleObject, Clone)]
#[graphql(name = "JanitorMatchedRule")]
pub struct MatchedRule {
    pub name: String,
    pub dry_run: bool,
    pub actions: Vec<RuleAction>,
}

impl From<janitor_bot::rules::MatchedRule> for MatchedRule {
    fn from(rule: janitor_bot::rules::MatchedRule) -> Self {
        Self {
            name: rule.name,
            dry_run: rule.dry_run,
            actions: rule
                .actions
                .into_iter()
                .map(|a| RuleAction {
                    action: a.action.to_owned(),
                    ran: a.ran,
                })
                .collect(),
        }
    }
}

#[derive(SimpleObject, Clone)]
#[graphql(name = "JanitorEvaluation")]
pub struct Evaluation {
    pub timestamp: DateTime<Utc>,
    pub event_kind: String,
    pub event_key: String,
    pub matched_rules: Vec<MatchedRule>,
    pub elapsed_ms: u64,
}

async fn evaluations(ctx: &Context<'_>) -> Vec<Evaluation> {
    state(ctx)
        .orchestrator
        .get_eval_log()
        .await
        .into_iter()
        .map(|e| Evaluation {
            timestamp: e.timestamp,
            event_kind: e.event_kind,
            event_key: e.event_key,
            matched_rules: e.matched_rules.into_iter().map(Into::into).collect(),
            elapsed_ms: e.elapsed_ms.try_into().unwrap_or(u64::MAX),
        })
        .collect()
}

#[derive(SimpleObject)]
#[graphql(name = "JanitorServiceHealth")]
pub struct ServiceHealth {
    pub service: String,
    pub healthy: bool,
    pub error: Option<String>,
}

pub struct QueryRoot;

#[Object(name = "Query")]
impl QueryRoot {
    async fn janitor_rules(&self, ctx: &Context<'_>) -> Vec<Rule> {
        rules(ctx)
    }

    /// Recent rule evaluations, oldest first.
    async fn janitor_evaluations(&self, ctx: &Context<'_>) -> Vec<Evaluation> {
        evaluations(ctx).await
    }

    /// Reachability of forgejo, github and argocd.
    async fn janitor_health(&self, ctx: &Context<'_>) -> Result<Vec<ServiceHealth>> {
        let (_, body) = routes::handle_admin_health_deep(State(state(ctx))).await;
        let services = body
            .0
            .get("services")
            .cloned()
            .unwrap_or(Value::Array(Vec::new()));
        Ok(services
            .as_array()
            .into_iter()
            .flatten()
            .map(|s| ServiceHealth {
                service: s["service"].as_str().unwrap_or_default().to_owned(),
                healthy: s["healthy"].as_bool().unwrap_or(false),
                error: s["error"].as_str().map(str::to_owned),
            })
            .collect())
    }

    /// Which rules would match the PR, and which actions would run, without running them.
    async fn janitor_dry_run(
        &self,
        ctx: &Context<'_>,
        owner: String,
        repo: String,
        number: i64,
    ) -> Result<Vec<MatchedRule>> {
        let matched = outcome(
            routes::handle_admin_dry_run(State(state(ctx)), Path((owner, repo, number))).await,
        )?;
        let matched: Vec<Value> = serde_json::from_value(matched.0)?;
        Ok(matched
            .into_iter()
            .map(|m| MatchedRule {
                name: m["name"].as_str().unwrap_or_default().to_owned(),
                dry_run: m["dry_run"].as_bool().unwrap_or(false),
                actions: m["actions"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|a| RuleAction {
                        action: a["action"].as_str().unwrap_or_default().to_owned(),
                        ran: a["ran"].as_bool().unwrap_or(false),
                    })
                    .collect(),
            })
            .collect())
    }

    #[graphql(entity)]
    async fn find_rule_by_id(&self, ctx: &Context<'_>, id: ID) -> Result<Rule> {
        let name = IDS.decode_as(&id, "rule")?;
        rules(ctx)
            .into_iter()
            .find(|r| r.name == name)
            .ok_or_else(|| Error::new(format!("rule {name} not found")))
    }
}

#[derive(Enum, Copy, Clone, Eq, PartialEq)]
#[graphql(name = "JanitorCommandTarget")]
pub enum CommandTarget {
    Pr,
    Issue,
}

#[derive(InputObject)]
#[graphql(name = "JanitorCommandInput")]
pub struct CommandInput {
    owner: String,
    repo: String,
    number: i64,
    target: CommandTarget,
    /// The comment body, e.g. `/janitor merge`.
    body: String,
}

pub struct MutationRoot;

#[Object(name = "Mutation")]
impl MutationRoot {
    /// Re-evaluates every open PR in the background.
    async fn janitor_run_cron(&self, ctx: &Context<'_>) -> bool {
        routes::handle_admin_cron(State(state(ctx)))
            .await
            .is_success()
    }

    /// Merges every PR labelled janitor/queued in the background.
    async fn janitor_merge_queued(&self, ctx: &Context<'_>) -> Result<bool> {
        outcome(routes::handle_admin_merge_queued(State(state(ctx))).await).map(|_| true)
    }

    /// Runs the PR's rules for real in the background.
    async fn janitor_evaluate_pr(
        &self,
        ctx: &Context<'_>,
        owner: String,
        repo: String,
        number: i64,
    ) -> bool {
        routes::handle_admin_evaluate_pr(State(state(ctx)), Path((owner, repo, number)))
            .await
            .is_success()
    }

    /// Dispatches a comment command as if it had been posted on the PR or issue.
    async fn janitor_command(&self, ctx: &Context<'_>, input: CommandInput) -> Result<Json<Value>> {
        let request = routes::AdminCommandRequest {
            owner: input.owner,
            repo: input.repo,
            number: input.number,
            kind: match input.target {
                CommandTarget::Pr => routes::AdminCommandKind::Pr,
                CommandTarget::Issue => routes::AdminCommandKind::Issue,
            },
            body: input.body,
        };
        outcome(routes::handle_admin_command(State(state(ctx)), axum::Json(request)).await)
    }

    async fn janitor_argocd_resync(&self, ctx: &Context<'_>, app: String) -> Result<bool> {
        outcome(routes::handle_admin_argocd_resync(State(state(ctx)), Path(app)).await)
            .map(|_| true)
    }
}

pub struct SubscriptionRoot;

const POLL_INTERVAL: Duration = Duration::from_secs(2);

#[Subscription(name = "Subscription")]
impl SubscriptionRoot {
    /// Rule evaluations as this replica records them.
    async fn janitor_evaluations(&self, ctx: &Context<'_>) -> impl Stream<Item = Evaluation> {
        let state = state(ctx);
        let since = state
            .orchestrator
            .get_eval_log()
            .await
            .last()
            .map(|e| e.timestamp);

        async_stream::stream! {
            let mut since = since;
            let mut ticker = tokio::time::interval(POLL_INTERVAL);
            loop {
                ticker.tick().await;
                for e in state.orchestrator.get_eval_log().await {
                    if since.is_some_and(|since| e.timestamp <= since) {
                        continue;
                    }
                    since = Some(e.timestamp);
                    yield Evaluation {
                        timestamp: e.timestamp,
                        event_kind: e.event_kind,
                        event_key: e.event_key,
                        matched_rules: e.matched_rules.into_iter().map(Into::into).collect(),
                        elapsed_ms: e.elapsed_ms.try_into().unwrap_or(u64::MAX),
                    };
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn subgraph_is_current() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("subgraph.graphql");
        subgraph::assert_subgraph_current(&super::builder().finish(), path);
    }
}
