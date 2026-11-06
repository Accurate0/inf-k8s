use async_graphql::{Context, ID, Object, Result};

use super::objects::{Change, ContextInput, Evaluation, Flag, Segment};
use super::{IDS, Services, grpc, request};
use crate::pb;
use crate::pb::admin_server::Admin;
use crate::pb::evaluation_server::Evaluation as _;

pub struct QueryRoot;

#[Object(name = "Query")]
impl QueryRoot {
    async fn feature_flags_flags(
        &self,
        ctx: &Context<'_>,
        #[graphql(default)] include_archived: bool,
    ) -> Result<Vec<Flag>> {
        let services = ctx.data_unchecked::<Services>();
        let flags = Admin::list_flags(
            &*services.admin,
            request(ctx, pb::ListFlagsRequest { include_archived })?,
        )
        .await
        .map_err(grpc)?;
        Ok(flags
            .into_inner()
            .flags
            .into_iter()
            .map(Into::into)
            .collect())
    }

    async fn feature_flags_flag(&self, ctx: &Context<'_>, key: String) -> Result<Flag> {
        Flag::load(ctx, &key).await
    }

    async fn feature_flags_segments(&self, ctx: &Context<'_>) -> Result<Vec<Segment>> {
        let services = ctx.data_unchecked::<Services>();
        let segments =
            Admin::list_segments(&*services.admin, request(ctx, pb::ListSegmentsRequest {})?)
                .await
                .map_err(grpc)?;
        Ok(segments
            .into_inner()
            .segments
            .into_iter()
            .map(Into::into)
            .collect())
    }

    async fn feature_flags_segment(&self, ctx: &Context<'_>, key: String) -> Result<Segment> {
        Segment::load(ctx, &key).await
    }

    /// The audit log, newest first; targetKind is `flag` or `segment`.
    async fn feature_flags_history(
        &self,
        ctx: &Context<'_>,
        target_kind: Option<String>,
        target_key: Option<String>,
        #[graphql(default = 100)] limit: u32,
    ) -> Result<Vec<Change>> {
        Change::list(
            ctx,
            target_kind.as_deref().unwrap_or_default(),
            target_key.as_deref().unwrap_or_default(),
            limit,
        )
        .await
    }

    async fn feature_flags_config_version(&self, ctx: &Context<'_>) -> i64 {
        ctx.data_unchecked::<Services>().mgr.version()
    }

    /// Evaluates every unarchived flag for the context, as a client would see it.
    async fn feature_flags_evaluate_all(
        &self,
        ctx: &Context<'_>,
        context: ContextInput,
    ) -> Result<Vec<Evaluation>> {
        let services = ctx.data_unchecked::<Services>();
        let resolved = services
            .evaluation
            .resolve_all(request(
                ctx,
                pb::ResolveAllRequest {
                    context: Some(context.into()),
                },
            )?)
            .await
            .map_err(grpc)?;
        Ok(resolved
            .into_inner()
            .flags
            .into_iter()
            .map(Into::into)
            .collect())
    }

    async fn feature_flags_evaluate(
        &self,
        ctx: &Context<'_>,
        flag_key: String,
        context: ContextInput,
    ) -> Result<Evaluation> {
        let all = self.feature_flags_evaluate_all(ctx, context).await?;
        all.into_iter()
            .find(|e| e.flag_key == flag_key)
            .ok_or_else(|| {
                grpc(tonic::Status::not_found(format!(
                    "flag {flag_key} not found"
                )))
            })
    }

    #[graphql(entity)]
    async fn find_flag_by_id(&self, ctx: &Context<'_>, id: ID) -> Result<Flag> {
        Flag::load(ctx, &IDS.decode_as(&id, "flag")?).await
    }

    #[graphql(entity)]
    async fn find_segment_by_id(&self, ctx: &Context<'_>, id: ID) -> Result<Segment> {
        Segment::load(ctx, &IDS.decode_as(&id, "segment")?).await
    }

    #[graphql(entity)]
    async fn find_change_by_id(&self, ctx: &Context<'_>, id: ID) -> Result<Change> {
        let key = IDS.decode_as(&id, "change")?;
        let uuid: uuid::Uuid = key
            .parse()
            .map_err(|_| async_graphql::Error::new(format!("invalid id {:?}", id.as_str())))?;
        let services = ctx.data_unchecked::<Services>();

        let change = sqlx::query_as!(
            crate::store::FlagChange,
            r#"SELECT id, version, actor, action, target_kind, target_key,
                      detail as "detail: serde_json::Value", created_at
               FROM flag_changes WHERE id = $1"#,
            uuid
        )
        .fetch_optional(services.store.pool())
        .await
        .map_err(|e| grpc(crate::error::AppError::from(e).into()))?
        .ok_or_else(|| grpc(tonic::Status::not_found(format!("change {uuid} not found"))))?;
        Ok(change.into())
    }
}
