use async_graphql::{Context, ID, InputObject, Object, Result};
use uuid::Uuid;

use super::chat::{self, ChatInput, ChatResult};
use super::objects::{CreatedKey, Key};
use super::{Gql, IDS, not_found, state};
use crate::keys::UpdateKey;
use crate::pricing;

pub struct MutationRoot;

#[derive(InputObject)]
#[graphql(name = "AiGatewayCreateKeyInput")]
pub struct CreateKeyInput {
    name: String,
    #[graphql(default)]
    allowed_models: Vec<String>,
    monthly_token_budget: Option<i64>,
}

#[derive(InputObject)]
#[graphql(name = "AiGatewayUpdateKeyInput")]
pub struct UpdateKeyInput {
    name: Option<String>,
    allowed_models: Option<Vec<String>>,
    monthly_token_budget: Option<i64>,
    revoked: Option<bool>,
}

#[derive(InputObject)]
#[graphql(name = "AiGatewayModelPriceInput")]
pub struct ModelPriceInput {
    model: String,
    input_usd_per_mtok: f64,
    output_usd_per_mtok: f64,
    cached_usd_per_mtok: Option<f64>,
}

fn key_id(id: &ID) -> Result<Uuid> {
    IDS.decode_as(id, "key")?
        .parse()
        .map_err(|_| async_graphql::Error::new(format!("invalid id {:?}", id.as_str())))
}

#[Object(name = "Mutation")]
impl MutationRoot {
    async fn ai_gateway_create_key(
        &self,
        ctx: &Context<'_>,
        input: CreateKeyInput,
    ) -> Result<CreatedKey> {
        let (secret, info) = state(ctx)
            .keys
            .create(
                &input.name,
                &input.allowed_models,
                input.monthly_token_budget,
            )
            .await
            .gql()?;
        Ok(CreatedKey {
            secret,
            key: info.into(),
        })
    }

    async fn ai_gateway_update_key(
        &self,
        ctx: &Context<'_>,
        id: ID,
        input: UpdateKeyInput,
    ) -> Result<Key> {
        let fields = UpdateKey {
            name: input.name,
            allowed_models: input.allowed_models,
            monthly_token_budget: input.monthly_token_budget,
            revoked: input.revoked,
        };
        state(ctx)
            .keys
            .update(key_id(&id)?, &fields)
            .await
            .gql()?
            .map(Into::into)
            .ok_or_else(|| not_found(format!("key {}", id.as_str())))
    }

    async fn ai_gateway_revoke_key(&self, ctx: &Context<'_>, id: ID) -> Result<Key> {
        let uuid = key_id(&id)?;
        if !state(ctx).keys.revoke(uuid).await.gql()? {
            return Err(not_found(format!("key {}", id.as_str())));
        }
        Key::load(ctx, uuid).await
    }

    /// Issues a new secret for the key; the old one stops working.
    async fn ai_gateway_regenerate_key(&self, ctx: &Context<'_>, id: ID) -> Result<CreatedKey> {
        let (secret, info) = state(ctx)
            .keys
            .regenerate(key_id(&id)?)
            .await
            .gql()?
            .ok_or_else(|| not_found(format!("key {}", id.as_str())))?;
        Ok(CreatedKey {
            secret,
            key: info.into(),
        })
    }

    /// Upserts prices and returns how many rows were written.
    async fn ai_gateway_upsert_prices(
        &self,
        ctx: &Context<'_>,
        prices: Vec<ModelPriceInput>,
    ) -> Result<u64> {
        let prices: Vec<pricing::ModelPrice> = prices
            .into_iter()
            .map(|p| pricing::ModelPrice {
                id: p.model,
                input_usd_per_mtok: p.input_usd_per_mtok,
                output_usd_per_mtok: p.output_usd_per_mtok,
                cached_usd_per_mtok: p.cached_usd_per_mtok,
            })
            .collect();
        let state = state(ctx);
        let written = pricing::upsert(&state.pool, &prices).await.gql()?;
        state.pricing.refresh(&state.pool).await.gql()?;
        Ok(written)
    }

    /// A chat completion billed to the caller's own key.
    async fn ai_gateway_chat(&self, ctx: &Context<'_>, input: ChatInput) -> Result<ChatResult> {
        chat::complete(ctx, input).await
    }
}
