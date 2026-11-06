use std::time::Duration;

use async_graphql::{Context, Result, Subscription};
use futures::Stream;

use super::chat::{self, ChatChunk, ChatInput};
use super::objects::UsageEvent;
use super::{Gql, state};
use crate::error::GatewayError;

pub struct SubscriptionRoot;

const POLL_INTERVAL: Duration = Duration::from_secs(2);

#[Subscription(name = "Subscription")]
impl SubscriptionRoot {
    /// Usage events as any replica records them.
    async fn ai_gateway_usage(
        &self,
        ctx: &Context<'_>,
    ) -> Result<impl Stream<Item = Result<UsageEvent>>> {
        let pool = state(ctx).pool.clone();
        let cursor =
            sqlx::query_scalar!(r#"SELECT COALESCE(MAX(id), 0) AS "id!" FROM usage_events"#)
                .fetch_one(&pool)
                .await
                .map_err(GatewayError::from)
                .gql()?;

        Ok(async_stream::try_stream! {
            let mut cursor = cursor;
            let mut ticker = tokio::time::interval(POLL_INTERVAL);
            loop {
                ticker.tick().await;
                let events = sqlx::query_as!(
                    UsageEvent,
                    "SELECT id AS row_id, key_id, key_name, provider, requested_model, \
                     resolved_model, input_tokens, output_tokens, latency_ms, status, cost_usd, \
                     cache_hit, created_at FROM usage_events \
                     WHERE id > $1 AND created_at > now() - interval '1 hour' \
                     ORDER BY id LIMIT 100",
                    cursor
                )
                .fetch_all(&pool)
                .await
                .map_err(GatewayError::from)
                .gql()?;

                for event in events {
                    cursor = event.row_id;
                    yield event;
                }
            }
        })
    }

    /// A streamed chat completion billed to the caller's own key.
    async fn ai_gateway_chat_stream(
        &self,
        ctx: &Context<'_>,
        input: ChatInput,
    ) -> Result<impl Stream<Item = Result<ChatChunk>>> {
        chat::stream(ctx, input).await
    }
}
