use async_graphql::{ComplexObject, Context, ID, Interface, Result, SimpleObject};
use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::{Gql, IDS, not_found, state};
use crate::keys::KeyInfo;

#[derive(Interface)]
#[graphql(field(name = "id", ty = "ID"))]
pub enum Node {
    Key(Key),
    Model(Model),
    ModelPrice(ModelPrice),
    UsageEvent(UsageEvent),
}

#[derive(SimpleObject)]
#[graphql(name = "AiGatewayKey", complex)]
pub struct Key {
    #[graphql(skip)]
    pub uuid: Uuid,
    pub name: String,
    /// Empty allows any model.
    pub allowed_models: Vec<String>,
    pub monthly_token_budget: Option<i64>,
    pub revoked: bool,
    pub created_at: DateTime<Utc>,
}

#[ComplexObject]
impl Key {
    async fn id(&self) -> ID {
        IDS.encode("key", self.uuid)
    }

    async fn month_to_date_tokens(&self, ctx: &Context<'_>) -> Result<i64> {
        state(ctx).keys.month_to_date_tokens(self.uuid).await.gql()
    }

    async fn usage_events(
        &self,
        ctx: &Context<'_>,
        #[graphql(default = 50)] limit: i64,
    ) -> Result<Vec<UsageEvent>> {
        UsageEvent::list(ctx, Some(self.uuid), limit).await
    }
}

impl From<KeyInfo> for Key {
    fn from(info: KeyInfo) -> Self {
        Self {
            uuid: info.id,
            name: info.name,
            allowed_models: info.allowed_models,
            monthly_token_budget: info.monthly_token_budget,
            revoked: info.revoked,
            created_at: info.created_at,
        }
    }
}

impl Key {
    pub async fn list(ctx: &Context<'_>) -> Result<Vec<Self>> {
        Ok(state(ctx)
            .keys
            .list()
            .await
            .gql()?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    pub async fn load(ctx: &Context<'_>, id: Uuid) -> Result<Self> {
        Self::list(ctx)
            .await?
            .into_iter()
            .find(|key| key.uuid == id)
            .ok_or_else(|| not_found(format!("key {id}")))
    }
}

#[derive(SimpleObject)]
#[graphql(name = "AiGatewayModel", complex)]
pub struct Model {
    pub name: String,
    pub provider: String,
}

#[ComplexObject]
impl Model {
    async fn id(&self) -> ID {
        IDS.encode("model", &self.name)
    }

    async fn price(&self, ctx: &Context<'_>) -> Result<Option<ModelPrice>> {
        ModelPrice::find(ctx, &self.name).await
    }
}

impl Model {
    pub fn list(ctx: &Context<'_>) -> Vec<Self> {
        let state = state(ctx);
        let mut models: std::collections::BTreeMap<String, String> =
            state.providers.models().into_iter().collect();
        state.config.advertise(&mut models);
        models
            .into_iter()
            .map(|(name, provider)| Self { name, provider })
            .collect()
    }
}

#[derive(SimpleObject)]
#[graphql(name = "AiGatewayModelPrice", complex)]
pub struct ModelPrice {
    pub model: String,
    pub input_usd_per_mtok: f64,
    pub output_usd_per_mtok: f64,
    pub cached_usd_per_mtok: Option<f64>,
    pub updated_at: DateTime<Utc>,
}

#[ComplexObject]
impl ModelPrice {
    async fn id(&self) -> ID {
        IDS.encode("modelPrice", &self.model)
    }
}

impl ModelPrice {
    pub async fn list(ctx: &Context<'_>) -> Result<Vec<Self>> {
        sqlx::query_as!(
            ModelPrice,
            "SELECT id AS model, input_usd_per_mtok, output_usd_per_mtok, cached_usd_per_mtok, \
             updated_at FROM model_prices ORDER BY id"
        )
        .fetch_all(&state(ctx).pool)
        .await
        .map_err(crate::error::GatewayError::from)
        .gql()
    }

    pub async fn find(ctx: &Context<'_>, model: &str) -> Result<Option<Self>> {
        sqlx::query_as!(
            ModelPrice,
            "SELECT id AS model, input_usd_per_mtok, output_usd_per_mtok, cached_usd_per_mtok, \
             updated_at FROM model_prices WHERE id = $1",
            model
        )
        .fetch_optional(&state(ctx).pool)
        .await
        .map_err(crate::error::GatewayError::from)
        .gql()
    }
}

#[derive(SimpleObject)]
#[graphql(name = "AiGatewayUsageEvent", complex)]
pub struct UsageEvent {
    #[graphql(skip)]
    pub row_id: i64,
    #[graphql(skip)]
    pub key_id: Option<Uuid>,
    pub key_name: String,
    pub provider: String,
    pub requested_model: String,
    pub resolved_model: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub latency_ms: i64,
    pub status: i32,
    pub cost_usd: f64,
    pub cache_hit: bool,
    pub created_at: DateTime<Utc>,
}

#[ComplexObject]
impl UsageEvent {
    async fn id(&self) -> ID {
        IDS.encode("usageEvent", self.row_id)
    }

    async fn key(&self, ctx: &Context<'_>) -> Result<Option<Key>> {
        match self.key_id {
            Some(id) => Key::load(ctx, id).await.map(Some),
            None => Ok(None),
        }
    }
}

impl UsageEvent {
    pub async fn list(ctx: &Context<'_>, key_id: Option<Uuid>, limit: i64) -> Result<Vec<Self>> {
        sqlx::query_as!(
            UsageEvent,
            "SELECT id AS row_id, key_id, key_name, provider, requested_model, resolved_model, \
             input_tokens, output_tokens, latency_ms, status, cost_usd, cache_hit, created_at \
             FROM usage_events WHERE ($1::uuid IS NULL OR key_id = $1) \
             ORDER BY created_at DESC LIMIT $2",
            key_id,
            limit.clamp(1, 1000)
        )
        .fetch_all(&state(ctx).pool)
        .await
        .map_err(crate::error::GatewayError::from)
        .gql()
    }

    pub async fn load(ctx: &Context<'_>, row_id: i64) -> Result<Self> {
        sqlx::query_as!(
            UsageEvent,
            "SELECT id AS row_id, key_id, key_name, provider, requested_model, resolved_model, \
             input_tokens, output_tokens, latency_ms, status, cost_usd, cache_hit, created_at \
             FROM usage_events WHERE id = $1",
            row_id
        )
        .fetch_optional(&state(ctx).pool)
        .await
        .map_err(crate::error::GatewayError::from)
        .gql()?
        .ok_or_else(|| not_found(format!("usage event {row_id}")))
    }
}

#[derive(SimpleObject)]
#[graphql(name = "AiGatewayUsageSummary")]
pub struct UsageSummary {
    pub key_name: String,
    pub model: String,
    pub requests: i64,
    pub cache_hits: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cost_usd: f64,
}

impl From<crate::usage::UsageRow> for UsageSummary {
    fn from(row: crate::usage::UsageRow) -> Self {
        Self {
            key_name: row.key_name,
            model: row.model,
            requests: row.requests,
            cache_hits: row.cache_hits,
            input_tokens: row.input_tokens,
            output_tokens: row.output_tokens,
            cost_usd: row.cost_usd,
        }
    }
}

#[derive(SimpleObject)]
#[graphql(name = "AiGatewayCreatedKey")]
pub struct CreatedKey {
    /// Shown once; only its hash is stored.
    pub secret: String,
    pub key: Key,
}
