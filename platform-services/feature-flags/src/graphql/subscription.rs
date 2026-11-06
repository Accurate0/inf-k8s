use async_graphql::{Context, Result, Subscription};
use futures::{Stream, StreamExt};

use super::objects::ConfigEvent;
use super::{Services, grpc, request};
use crate::pb;
use crate::pb::evaluation_server::Evaluation;

pub struct SubscriptionRoot;

#[Subscription(name = "Subscription")]
impl SubscriptionRoot {
    /// A READY event, then one per config change; RESYNC means events were dropped.
    async fn feature_flags_changes(
        &self,
        ctx: &Context<'_>,
    ) -> Result<impl Stream<Item = ConfigEvent>> {
        let services = ctx.data_unchecked::<Services>();
        let events = services
            .evaluation
            .stream_events(request(ctx, pb::EventStreamRequest {})?)
            .await
            .map_err(grpc)?
            .into_inner();

        Ok(events.filter_map(|event| async move {
            let event = event.ok()?;
            Some(ConfigEvent {
                event_type: pb::EventType::try_from(event.r#type)
                    .unwrap_or(pb::EventType::Unspecified)
                    .into(),
                config_version: event.config_version,
                changed_flag_keys: event.changed_flag_keys,
            })
        }))
    }
}
