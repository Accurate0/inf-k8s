use async_graphql::{Context, ID, InputObject, Object, Result};

use super::objects::{
    ApplyResult, Flag, RuleInput, Segment, SegmentInput, ValueType, VariantInput,
};
use super::{IDS, Services, grpc, request};
use crate::pb;
use crate::pb::admin_server::Admin;

pub struct MutationRoot;

#[derive(InputObject)]
#[graphql(name = "FeatureFlagsCreateFlagInput")]
pub struct CreateFlagInput {
    key: String,
    value_type: ValueType,
    #[graphql(default)]
    enabled: bool,
    default_variant_key: String,
    variants: Vec<VariantInput>,
}

#[derive(InputObject)]
#[graphql(name = "FeatureFlagsFlagInput")]
pub struct FlagInput {
    key: String,
    value_type: ValueType,
    enabled: bool,
    default_variant_key: String,
    #[graphql(default)]
    archived: bool,
    variants: Vec<VariantInput>,
    #[graphql(default)]
    rules: Vec<RuleInput>,
}

impl From<FlagInput> for pb::Flag {
    fn from(input: FlagInput) -> Self {
        Self {
            key: input.key,
            value_type: pb::ValueType::from(input.value_type) as i32,
            enabled: input.enabled,
            default_variant_key: input.default_variant_key,
            archived: input.archived,
            variants: input.variants.into_iter().map(Into::into).collect(),
            rules: input.rules.into_iter().map(Into::into).collect(),
        }
    }
}

fn admin<'a>(ctx: &Context<'a>) -> &'a crate::grpc::AdminService {
    &ctx.data_unchecked::<Services>().admin
}

#[Object(name = "Mutation")]
impl MutationRoot {
    async fn feature_flags_create_flag(
        &self,
        ctx: &Context<'_>,
        input: CreateFlagInput,
    ) -> Result<Flag> {
        let message = pb::CreateFlagRequest {
            key: input.key,
            value_type: pb::ValueType::from(input.value_type) as i32,
            enabled: input.enabled,
            default_variant_key: input.default_variant_key,
            variants: input.variants.into_iter().map(Into::into).collect(),
        };
        let flag = Admin::create_flag(admin(ctx), request(ctx, message)?)
            .await
            .map_err(grpc)?;
        Ok(flag.into_inner().into())
    }

    async fn feature_flags_update_flag(
        &self,
        ctx: &Context<'_>,
        key: String,
        enabled: bool,
        default_variant_key: String,
    ) -> Result<Flag> {
        let message = pb::UpdateFlagRequest {
            key,
            enabled,
            default_variant_key,
        };
        let flag = Admin::update_flag(admin(ctx), request(ctx, message)?)
            .await
            .map_err(grpc)?;
        Ok(flag.into_inner().into())
    }

    /// Flips `enabled` without touching the default variant.
    async fn feature_flags_toggle_flag(
        &self,
        ctx: &Context<'_>,
        key: String,
        enabled: bool,
    ) -> Result<Flag> {
        let current = Flag::load(ctx, &key).await?;
        self.feature_flags_update_flag(ctx, key, enabled, current.default_variant_key)
            .await
    }

    async fn feature_flags_archive_flag(
        &self,
        ctx: &Context<'_>,
        key: String,
        #[graphql(default = true)] archived: bool,
    ) -> Result<Flag> {
        let flag = Admin::archive_flag(
            admin(ctx),
            request(ctx, pb::ArchiveFlagRequest { key, archived })?,
        )
        .await
        .map_err(grpc)?;
        Ok(flag.into_inner().into())
    }

    async fn feature_flags_delete_flag(&self, ctx: &Context<'_>, key: String) -> Result<ID> {
        Admin::delete_flag(
            admin(ctx),
            request(ctx, pb::DeleteFlagRequest { key: key.clone() })?,
        )
        .await
        .map_err(grpc)?;
        Ok(IDS.encode("flag", key))
    }

    async fn feature_flags_upsert_variant(
        &self,
        ctx: &Context<'_>,
        flag_key: String,
        variant: VariantInput,
    ) -> Result<Flag> {
        let message = pb::UpsertVariantRequest {
            flag_key,
            variant: Some(variant.into()),
        };
        let flag = Admin::upsert_variant(admin(ctx), request(ctx, message)?)
            .await
            .map_err(grpc)?;
        Ok(flag.into_inner().into())
    }

    async fn feature_flags_delete_variant(
        &self,
        ctx: &Context<'_>,
        flag_key: String,
        variant_key: String,
    ) -> Result<Flag> {
        let message = pb::DeleteVariantRequest {
            flag_key,
            variant_key,
        };
        let flag = Admin::delete_variant(admin(ctx), request(ctx, message)?)
            .await
            .map_err(grpc)?;
        Ok(flag.into_inner().into())
    }

    /// Replaces every rule on the flag.
    async fn feature_flags_set_flag_rules(
        &self,
        ctx: &Context<'_>,
        flag_key: String,
        rules: Vec<RuleInput>,
    ) -> Result<Flag> {
        let message = pb::SetFlagRulesRequest {
            flag_key,
            rules: rules.into_iter().map(Into::into).collect(),
        };
        let flag = Admin::set_flag_rules(admin(ctx), request(ctx, message)?)
            .await
            .map_err(grpc)?;
        Ok(flag.into_inner().into())
    }

    async fn feature_flags_create_segment(
        &self,
        ctx: &Context<'_>,
        input: SegmentInput,
    ) -> Result<Segment> {
        let message = pb::CreateSegmentRequest {
            segment: Some(input.into()),
        };
        let segment = Admin::create_segment(admin(ctx), request(ctx, message)?)
            .await
            .map_err(grpc)?;
        Ok(segment.into_inner().into())
    }

    async fn feature_flags_update_segment(
        &self,
        ctx: &Context<'_>,
        input: SegmentInput,
    ) -> Result<Segment> {
        let message = pb::UpdateSegmentRequest {
            segment: Some(input.into()),
        };
        let segment = Admin::update_segment(admin(ctx), request(ctx, message)?)
            .await
            .map_err(grpc)?;
        Ok(segment.into_inner().into())
    }

    async fn feature_flags_delete_segment(&self, ctx: &Context<'_>, key: String) -> Result<ID> {
        Admin::delete_segment(
            admin(ctx),
            request(ctx, pb::DeleteSegmentRequest { key: key.clone() })?,
        )
        .await
        .map_err(grpc)?;
        Ok(IDS.encode("segment", key))
    }

    /// Reconciles the whole config to these flags and segments, like `ffctl apply`.
    async fn feature_flags_apply_config(
        &self,
        ctx: &Context<'_>,
        flags: Vec<FlagInput>,
        segments: Vec<SegmentInput>,
        #[graphql(default)] dry_run: bool,
        #[graphql(default)] expected_version: i64,
    ) -> Result<ApplyResult> {
        let message = pb::ApplyConfigRequest {
            flags: flags.into_iter().map(Into::into).collect(),
            segments: segments.into_iter().map(Into::into).collect(),
            dry_run,
            expected_version,
        };
        let outcome = Admin::apply_config(admin(ctx), request(ctx, message)?)
            .await
            .map_err(grpc)?
            .into_inner();
        Ok(ApplyResult {
            changes: outcome.changes.into_iter().map(Into::into).collect(),
            from_version: outcome.from_version,
            to_version: outcome.to_version,
            applied: outcome.applied,
        })
    }
}
