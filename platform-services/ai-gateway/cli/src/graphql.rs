use chrono::{DateTime, Utc};
use serde::Serialize;

#[cynic::schema("graph")]
mod schema {}

cynic::impl_scalar!(DateTime<Utc>, schema::DateTime);

#[derive(cynic::QueryFragment, Serialize, Debug)]
#[cynic(graphql_type = "AiGatewayKey")]
pub struct Key {
    pub id: cynic::Id,
    pub name: String,
    pub allowed_models: Vec<String>,
    pub monthly_token_budget: Option<i32>,
    pub month_to_date_tokens: i32,
    pub revoked: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(cynic::QueryFragment, Serialize, Debug)]
#[cynic(graphql_type = "AiGatewayCreatedKey")]
pub struct CreatedKey {
    /// Shown once.
    pub secret: String,
    pub key: Key,
}

#[derive(cynic::QueryFragment, Serialize, Debug)]
#[cynic(graphql_type = "AiGatewayModelPrice")]
pub struct Price {
    pub input_usd_per_mtok: f64,
    pub output_usd_per_mtok: f64,
    pub cached_usd_per_mtok: Option<f64>,
}

#[derive(cynic::QueryFragment, Serialize, Debug)]
#[cynic(graphql_type = "AiGatewayModel")]
pub struct Model {
    pub name: String,
    pub provider: String,
    pub price: Option<Price>,
}

#[derive(cynic::QueryFragment, Serialize, Debug)]
#[cynic(graphql_type = "AiGatewayUsageSummary")]
pub struct UsageSummary {
    pub key_name: String,
    pub model: String,
    pub requests: i32,
    pub cache_hits: i32,
    pub input_tokens: i32,
    pub output_tokens: i32,
    pub cost_usd: f64,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Query")]
pub struct KeysQuery {
    pub ai_gateway_keys: Option<Vec<Key>>,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Query")]
pub struct ModelsQuery {
    pub ai_gateway_models: Option<Vec<Model>>,
}

#[derive(cynic::QueryVariables, Debug)]
pub struct UsageVariables {
    pub since: String,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Query", variables = "UsageVariables")]
pub struct UsageQuery {
    #[arguments(since: $since)]
    pub ai_gateway_usage: Option<Vec<UsageSummary>>,
}

#[derive(cynic::InputObject, Debug)]
#[cynic(graphql_type = "AiGatewayCreateKeyInput")]
pub struct CreateKeyInput {
    pub name: String,
    pub allowed_models: Vec<String>,
    pub monthly_token_budget: Option<i32>,
}

#[derive(cynic::QueryVariables, Debug)]
pub struct CreateKeyVariables {
    pub input: CreateKeyInput,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Mutation", variables = "CreateKeyVariables")]
pub struct CreateKey {
    #[arguments(input: $input)]
    pub ai_gateway_create_key: CreatedKey,
}

#[derive(cynic::InputObject, Debug)]
#[cynic(graphql_type = "AiGatewayUpdateKeyInput")]
pub struct UpdateKeyInput {
    pub name: Option<String>,
    pub allowed_models: Option<Vec<String>>,
    pub monthly_token_budget: Option<i32>,
    pub revoked: Option<bool>,
}

#[derive(cynic::QueryVariables, Debug)]
pub struct UpdateKeyVariables {
    pub id: cynic::Id,
    pub input: UpdateKeyInput,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Mutation", variables = "UpdateKeyVariables")]
pub struct UpdateKey {
    #[arguments(id: $id, input: $input)]
    pub ai_gateway_update_key: Key,
}

#[derive(cynic::QueryVariables, Debug)]
pub struct KeyIdVariables {
    pub id: cynic::Id,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Mutation", variables = "KeyIdVariables")]
pub struct RevokeKey {
    #[arguments(id: $id)]
    pub ai_gateway_revoke_key: Key,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Mutation", variables = "KeyIdVariables")]
pub struct RegenerateKey {
    #[arguments(id: $id)]
    pub ai_gateway_regenerate_key: CreatedKey,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Mutation")]
pub struct SyncPrices {
    pub ai_gateway_sync_prices: i32,
}
