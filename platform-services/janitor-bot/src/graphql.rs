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
        .extension(subgraph::Tracing)
        .extension(subgraph::Scopes)
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

#[derive(SimpleObject)]
#[graphql(name = "JanitorMetricLabel")]
pub struct MetricLabel {
    name: String,
    value: String,
}

/// One Prometheus sample from this replica's recorder.
#[derive(SimpleObject)]
#[graphql(name = "JanitorMetricSample")]
pub struct MetricSample {
    name: String,
    labels: Vec<MetricLabel>,
    value: f64,
}

/// Parses the Prometheus text exposition format, skipping comments and unparsable lines.
fn parse_metrics(text: &str) -> Vec<MetricSample> {
    text.lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| {
            let (series, value) = line.rsplit_once(' ')?;
            let value = value.parse().ok()?;
            let (name, labels) = match series.split_once('{') {
                Some((name, rest)) => (name, rest.strip_suffix('}')?),
                None => (series, ""),
            };
            let labels = labels
                .split(',')
                .filter(|pair| !pair.is_empty())
                .filter_map(|pair| {
                    let (name, value) = pair.split_once('=')?;
                    Some(MetricLabel {
                        name: name.to_owned(),
                        value: value.trim_matches('"').to_owned(),
                    })
                })
                .collect();
            Some(MetricSample {
                name: name.to_owned(),
                labels,
                value,
            })
        })
        .collect()
}

#[derive(Enum, Clone, Copy, PartialEq, Eq)]
#[graphql(name = "JanitorWebhookSource")]
pub enum WebhookSource {
    Forgejo,
    Github,
    Flux,
}

pub struct QueryRoot;

#[Object(name = "Query")]
impl QueryRoot {
    #[graphql(requires_scopes = "janitor:rule:read")]
    async fn janitor_rules(&self, ctx: &Context<'_>) -> Vec<Rule> {
        rules(ctx)
    }

    /// This replica's counters and histograms, as Prometheus scrapes them.
    #[graphql(requires_scopes = "janitor:rule:read")]
    async fn janitor_metrics(&self) -> Vec<MetricSample> {
        parse_metrics(&janitor_bot::metrics::render())
    }

    /// Recent rule evaluations, oldest first.
    #[graphql(requires_scopes = "janitor:rule:read")]
    async fn janitor_evaluations(&self, ctx: &Context<'_>) -> Vec<Evaluation> {
        evaluations(ctx).await
    }

    /// Reachability of forgejo and github.
    #[graphql(requires_scopes = "janitor:rule:read")]
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
    #[graphql(requires_scopes = "janitor:rule:read")]
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
    #[graphql(requires_scopes = "janitor:action:run")]
    async fn janitor_run_cron(&self, ctx: &Context<'_>) -> bool {
        routes::handle_admin_cron(State(state(ctx)))
            .await
            .is_success()
    }

    /// Merges every PR labelled janitor/queued in the background.
    #[graphql(requires_scopes = "janitor:action:run")]
    async fn janitor_merge_queued(&self, ctx: &Context<'_>) -> Result<bool> {
        outcome(routes::handle_admin_merge_queued(State(state(ctx))).await).map(|_| true)
    }

    /// Runs the PR's rules for real in the background.
    #[graphql(requires_scopes = "janitor:action:run")]
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
    #[graphql(requires_scopes = "janitor:action:run")]
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

    /// Feeds a saved webhook payload through its handler as if the source had sent it; the
    /// event is inferred from the payload when left out. Returns the handler's HTTP status.
    #[graphql(requires_scopes = "janitor:webhook:replay")]
    async fn janitor_replay_webhook(
        &self,
        ctx: &Context<'_>,
        source: WebhookSource,
        event: Option<String>,
        event_type: Option<String>,
        payload: String,
    ) -> Result<i32> {
        use axum::http::{HeaderMap, HeaderValue};
        use janitor_webhook::{forgejo, github};

        let state = state(ctx);
        let body = axum::body::Bytes::from(payload.into_bytes());
        let header = |value: &str| {
            HeaderValue::from_str(value).map_err(|e| Error::new(format!("invalid header: {e}")))
        };
        let mut headers = HeaderMap::new();

        let status = match source {
            WebhookSource::Github => {
                let event = match event {
                    Some(event) => event,
                    None => github::infer_event(&body)
                        .ok_or_else(|| Error::new("could not infer the github event; pass event"))?
                        .to_owned(),
                };
                headers.insert("X-GitHub-Event", header(&event)?);
                headers.insert(
                    "X-Hub-Signature-256",
                    header(&github::sign_payload(&state.github_webhook_secret, &body))?,
                );
                routes::handle_github_webhook(State(state), headers, body).await
            }
            WebhookSource::Forgejo => {
                let (event, event_type) = match event {
                    Some(event) => (event, event_type),
                    None => {
                        let (event, inferred) = forgejo::infer_event(&body).ok_or_else(|| {
                            Error::new("could not infer the forgejo event; pass event")
                        })?;
                        (event.to_owned(), event_type.or(inferred.map(str::to_owned)))
                    }
                };
                headers.insert("Authorization", header(&state.forgejo_webhook_secret)?);
                headers.insert("X-Forgejo-Event", header(&event)?);
                if let Some(event_type) = event_type {
                    headers.insert("X-Forgejo-Event-Type", header(&event_type)?);
                }
                routes::handle_forgejo_webhook(State(state), headers, body).await
            }
            WebhookSource::Flux => {
                headers.insert(
                    "X-Signature",
                    header(&github::sign_payload(&state.flux_webhook_secret, &body))?,
                );
                routes::handle_flux_webhook(State(state), headers, body).await
            }
        };

        Ok(i32::from(status.as_u16()))
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
    fn prometheus_text_parses_into_samples() {
        let samples = super::parse_metrics(
            "# TYPE janitor_webhooks_total counter\n\
             janitor_webhooks_total{source=\"github\"} 3\n\
             janitor_cron_runs_total 1\n\
             garbage\n",
        );
        assert_eq!(samples.len(), 2);
        assert_eq!(samples[0].name, "janitor_webhooks_total");
        assert_eq!(samples[0].labels[0].name, "source");
        assert_eq!(samples[0].labels[0].value, "github");
        assert_eq!(samples[0].value, 3.0);
        assert!(samples[1].labels.is_empty());
    }

    #[test]
    fn subgraph_is_current() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("subgraph.graphql");
        subgraph::assert_subgraph_current(&super::builder().finish(), path);
    }
}
