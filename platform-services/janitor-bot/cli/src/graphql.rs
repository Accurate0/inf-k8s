use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;

#[cynic::schema("graph")]
mod schema {}

cynic::impl_scalar!(DateTime<Utc>, schema::DateTime);
cynic::impl_scalar!(Value, schema::JSON);

#[derive(cynic::QueryFragment, Serialize, Debug)]
#[cynic(graphql_type = "JanitorRule")]
pub struct Rule {
    pub name: String,
    pub enabled: String,
    pub priority: i32,
    pub depends_on: Vec<String>,
}

#[derive(cynic::QueryFragment, Serialize, Debug)]
#[cynic(graphql_type = "JanitorRuleAction")]
pub struct RuleAction {
    pub action: String,
    pub ran: bool,
}

#[derive(cynic::QueryFragment, Serialize, Debug)]
#[cynic(graphql_type = "JanitorMatchedRule")]
pub struct MatchedRule {
    pub name: String,
    pub dry_run: bool,
    pub actions: Vec<RuleAction>,
}

#[derive(cynic::QueryFragment, Serialize, Debug)]
#[cynic(graphql_type = "JanitorEvaluation")]
pub struct Evaluation {
    pub timestamp: DateTime<Utc>,
    pub event_kind: String,
    pub event_key: String,
    pub matched_rules: Vec<MatchedRule>,
    pub elapsed_ms: i32,
}

#[derive(cynic::QueryFragment, Serialize, Debug)]
#[cynic(graphql_type = "JanitorServiceHealth")]
pub struct ServiceHealth {
    pub service: String,
    pub healthy: bool,
    pub error: Option<String>,
}

#[derive(cynic::QueryFragment, Serialize, Debug)]
#[cynic(graphql_type = "JanitorMetricLabel")]
pub struct MetricLabel {
    pub name: String,
    pub value: String,
}

#[derive(cynic::QueryFragment, Serialize, Debug)]
#[cynic(graphql_type = "JanitorMetricSample")]
pub struct MetricSample {
    pub name: String,
    pub labels: Vec<MetricLabel>,
    pub value: f64,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Query")]
pub struct RulesQuery {
    pub janitor_rules: Vec<Rule>,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Query")]
pub struct EvaluationsQuery {
    pub janitor_evaluations: Vec<Evaluation>,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Query")]
pub struct HealthQuery {
    pub janitor_health: Vec<ServiceHealth>,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Query")]
pub struct MetricsQuery {
    pub janitor_metrics: Vec<MetricSample>,
}

#[derive(cynic::QueryVariables, Debug)]
pub struct PrVariables {
    pub owner: String,
    pub repo: String,
    pub number: i32,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Query", variables = "PrVariables")]
pub struct DryRunQuery {
    #[arguments(owner: $owner, repo: $repo, number: $number)]
    pub janitor_dry_run: Vec<MatchedRule>,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Mutation", variables = "PrVariables")]
pub struct EvaluatePr {
    #[arguments(owner: $owner, repo: $repo, number: $number)]
    pub janitor_evaluate_pr: bool,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Mutation")]
pub struct RunCron {
    pub janitor_run_cron: bool,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Mutation")]
pub struct MergeQueued {
    pub janitor_merge_queued: bool,
}

#[derive(cynic::Enum, Clone, Copy, Debug)]
#[cynic(graphql_type = "JanitorCommandTarget")]
pub enum CommandTarget {
    Pr,
    Issue,
}

#[derive(cynic::InputObject, Debug)]
#[cynic(graphql_type = "JanitorCommandInput")]
pub struct CommandInput {
    pub owner: String,
    pub repo: String,
    pub number: i32,
    pub target: CommandTarget,
    pub body: String,
}

#[derive(cynic::QueryVariables, Debug)]
pub struct CommandVariables {
    pub input: CommandInput,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Mutation", variables = "CommandVariables")]
pub struct Dispatch {
    #[arguments(input: $input)]
    pub janitor_command: Value,
}

#[derive(cynic::Enum, Clone, Copy, Debug)]
#[cynic(graphql_type = "JanitorWebhookSource")]
pub enum WebhookSource {
    Forgejo,
    Github,
    Flux,
}

#[derive(cynic::QueryVariables, Debug)]
pub struct ReplayVariables {
    pub source: WebhookSource,
    pub event: Option<String>,
    pub event_type: Option<String>,
    pub payload: String,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Mutation", variables = "ReplayVariables")]
pub struct Replay {
    #[arguments(source: $source, event: $event, eventType: $event_type, payload: $payload)]
    pub janitor_replay_webhook: i32,
}
