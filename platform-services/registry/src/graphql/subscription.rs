use std::time::Duration;

use async_graphql::{Context, Result, Subscription};
use futures::Stream;

use super::objects::AuditEntry;
use super::{Gql, principal, state};
use crate::error::ApiError;

pub struct SubscriptionRoot;

const POLL_INTERVAL: Duration = Duration::from_secs(2);

#[Subscription(name = "Subscription")]
impl SubscriptionRoot {
    /// New audit entries as they are recorded by any replica.
    async fn registry_audit(
        &self,
        ctx: &Context<'_>,
    ) -> Result<impl Stream<Item = Result<AuditEntry>>> {
        principal(ctx)?.require_admin().gql()?;
        let pool = state(ctx).pool.clone();

        let cursor =
            sqlx::query_scalar!(r#"SELECT COALESCE(MAX(id), 0) AS "id!" FROM core_audit_log"#)
                .fetch_one(&pool)
                .await
                .map_err(ApiError::from)
                .gql()?;

        Ok(async_stream::try_stream! {
            let mut cursor = cursor;
            let mut ticker = tokio::time::interval(POLL_INTERVAL);
            loop {
                ticker.tick().await;
                let rows = sqlx::query_as!(
                    registry_api::AuditEntry,
                    "SELECT id, at, actor, actor_kind, action, ecosystem, package, version, detail
                     FROM core_audit_log WHERE id > $1 ORDER BY id LIMIT 100",
                    cursor
                )
                .fetch_all(&pool)
                .await
                .map_err(ApiError::from)
                .gql()?;

                for row in rows {
                    cursor = row.id;
                    yield AuditEntry::from(row);
                }
            }
        })
    }
}
