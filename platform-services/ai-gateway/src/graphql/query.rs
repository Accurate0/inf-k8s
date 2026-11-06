use async_graphql::{Context, ID, Object, Result};
use uuid::Uuid;

use super::objects::{Key, Model, ModelPrice, UsageEvent, UsageSummary};
use super::{Gql, IDS, not_found, state, user_key};
use crate::error::GatewayError;
use crate::usage::{self, Window};

pub struct QueryRoot;

fn uuid(key: &str) -> Result<Uuid> {
    key.parse()
        .map_err(|_| async_graphql::Error::new(format!("invalid key id {key:?}")))
}

#[Object(name = "Query")]
impl QueryRoot {
    async fn ai_gateway_keys(&self, ctx: &Context<'_>) -> Result<Vec<Key>> {
        Key::list(ctx).await
    }

    /// The caller's own key, used for graph chat; minted on first use.
    async fn ai_gateway_my_key(&self, ctx: &Context<'_>) -> Result<Key> {
        Key::load(ctx, user_key(ctx).await?.id).await
    }

    async fn ai_gateway_models(&self, ctx: &Context<'_>) -> Vec<Model> {
        Model::list(ctx)
    }

    async fn ai_gateway_prices(&self, ctx: &Context<'_>) -> Result<Vec<ModelPrice>> {
        ModelPrice::list(ctx).await
    }

    /// Usage per key and model over a window such as `24h`, `7d` or `30d`.
    async fn ai_gateway_usage(
        &self,
        ctx: &Context<'_>,
        #[graphql(default = "30d")] since: String,
    ) -> Result<Vec<UsageSummary>> {
        let window: Window = since.parse().map_err(GatewayError::BadRequest).gql()?;
        Ok(usage::summary(&state(ctx).pool, window)
            .await
            .gql()?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    async fn ai_gateway_usage_events(
        &self,
        ctx: &Context<'_>,
        key_id: Option<ID>,
        #[graphql(default = 100)] limit: i64,
    ) -> Result<Vec<UsageEvent>> {
        let key_id = match key_id {
            Some(id) => Some(uuid(&IDS.decode_as(&id, "key")?)?),
            None => None,
        };
        UsageEvent::list(ctx, key_id, limit).await
    }

    #[graphql(entity)]
    async fn find_key_by_id(&self, ctx: &Context<'_>, id: ID) -> Result<Key> {
        Key::load(ctx, uuid(&IDS.decode_as(&id, "key")?)?).await
    }

    #[graphql(entity)]
    async fn find_model_by_id(&self, ctx: &Context<'_>, id: ID) -> Result<Model> {
        let name = IDS.decode_as(&id, "model")?;
        Model::list(ctx)
            .into_iter()
            .find(|model| model.name == name)
            .ok_or_else(|| not_found(format!("model {name}")))
    }

    #[graphql(entity)]
    async fn find_model_price_by_id(&self, ctx: &Context<'_>, id: ID) -> Result<ModelPrice> {
        let model = IDS.decode_as(&id, "modelPrice")?;
        ModelPrice::find(ctx, &model)
            .await?
            .ok_or_else(|| not_found(format!("price for {model}")))
    }

    #[graphql(entity)]
    async fn find_usage_event_by_id(&self, ctx: &Context<'_>, id: ID) -> Result<UsageEvent> {
        UsageEvent::load(ctx, IDS.decode_row_id(&id, "usageEvent")?).await
    }
}
