use async_graphql::{
    ComplexObject, Context, Enum, ID, InputObject, Interface, Json, Result, SimpleObject,
};
use chrono::{DateTime, Utc};
use serde_json::Value;

use super::{IDS, Services};
use crate::convert::{json_to_prost_value, prost_value_to_json};
use crate::pb;
use crate::pb::admin_server::Admin;

#[derive(Interface)]
#[graphql(field(name = "id", ty = "ID"))]
pub enum Node {
    Flag(Flag),
    Segment(Segment),
    Change(Change),
}

#[derive(Enum, Copy, Clone, Eq, PartialEq)]
#[graphql(name = "FeatureFlagsValueType", remote = "pb::ValueType")]
pub enum ValueType {
    Unspecified,
    Boolean,
    String,
    Integer,
    Float,
    Object,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq)]
#[graphql(name = "FeatureFlagsReason", remote = "pb::Reason")]
pub enum Reason {
    Unspecified,
    Static,
    Default,
    TargetingMatch,
    Split,
    Disabled,
    Error,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq)]
#[graphql(
    name = "FeatureFlagsConstraintOperator",
    remote = "pb::ConstraintOperator"
)]
pub enum ConstraintOperator {
    Unspecified,
    Eq,
    Neq,
    In,
    NotIn,
    Contains,
    StartsWith,
    EndsWith,
    Gt,
    Gte,
    Lt,
    Lte,
    Exists,
    Regex,
    FlagMatches,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq)]
#[graphql(name = "FeatureFlagsEventType", remote = "pb::EventType")]
pub enum EventType {
    Unspecified,
    Ready,
    ConfigurationChanged,
    Resync,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq)]
#[graphql(name = "FeatureFlagsChangeOp", remote = "pb::ChangeOp")]
pub enum ChangeOp {
    Unspecified,
    Create,
    Update,
    Delete,
}

fn value_type(raw: i32) -> ValueType {
    pb::ValueType::try_from(raw)
        .unwrap_or(pb::ValueType::Unspecified)
        .into()
}

fn json(value: &Option<prost_types::Value>) -> Json<Value> {
    Json(
        value
            .as_ref()
            .map(prost_value_to_json)
            .unwrap_or(Value::Null),
    )
}

#[derive(SimpleObject)]
#[graphql(name = "FeatureFlagsFlag", complex)]
pub struct Flag {
    pub key: String,
    pub value_type: ValueType,
    pub enabled: bool,
    pub default_variant_key: String,
    pub archived: bool,
    pub variants: Vec<Variant>,
    pub rules: Vec<Rule>,
}

#[ComplexObject]
impl Flag {
    async fn id(&self) -> ID {
        IDS.encode("flag", &self.key)
    }

    async fn history(
        &self,
        ctx: &Context<'_>,
        #[graphql(default = 50)] limit: u32,
    ) -> Result<Vec<Change>> {
        Change::list(ctx, "flag", &self.key, limit).await
    }
}

impl From<pb::Flag> for Flag {
    fn from(flag: pb::Flag) -> Self {
        Self {
            value_type: value_type(flag.value_type),
            key: flag.key,
            enabled: flag.enabled,
            default_variant_key: flag.default_variant_key,
            archived: flag.archived,
            variants: flag.variants.into_iter().map(Into::into).collect(),
            rules: flag.rules.into_iter().map(Into::into).collect(),
        }
    }
}

impl Flag {
    pub async fn load(ctx: &Context<'_>, key: &str) -> Result<Self> {
        let services = ctx.data_unchecked::<Services>();
        let request = super::request(
            ctx,
            pb::GetFlagRequest {
                key: key.to_owned(),
            },
        )?;
        let flag = Admin::get_flag(&*services.admin, request)
            .await
            .map_err(super::grpc)?;
        Ok(flag.into_inner().into())
    }
}

#[derive(SimpleObject)]
#[graphql(name = "FeatureFlagsVariant")]
pub struct Variant {
    pub key: String,
    pub value: Json<Value>,
}

impl From<pb::Variant> for Variant {
    fn from(variant: pb::Variant) -> Self {
        Self {
            value: json(&variant.value),
            key: variant.key,
        }
    }
}

#[derive(InputObject)]
#[graphql(name = "FeatureFlagsVariantInput")]
pub struct VariantInput {
    pub key: String,
    pub value: Json<Value>,
}

impl From<VariantInput> for pb::Variant {
    fn from(input: VariantInput) -> Self {
        Self {
            key: input.key,
            value: Some(json_to_prost_value(&input.value.0)),
        }
    }
}

#[derive(SimpleObject)]
#[graphql(name = "FeatureFlagsConstraint")]
pub struct Constraint {
    pub attribute: String,
    pub operator: ConstraintOperator,
    pub values: Vec<Json<Value>>,
}

impl From<pb::Constraint> for Constraint {
    fn from(constraint: pb::Constraint) -> Self {
        Self {
            operator: pb::ConstraintOperator::try_from(constraint.operator)
                .unwrap_or(pb::ConstraintOperator::Unspecified)
                .into(),
            attribute: constraint.attribute,
            values: constraint
                .values
                .iter()
                .map(|v| Json(prost_value_to_json(v)))
                .collect(),
        }
    }
}

#[derive(InputObject)]
#[graphql(name = "FeatureFlagsConstraintInput")]
pub struct ConstraintInput {
    pub attribute: String,
    pub operator: ConstraintOperator,
    #[graphql(default)]
    pub values: Vec<Json<Value>>,
}

impl From<ConstraintInput> for pb::Constraint {
    fn from(input: ConstraintInput) -> Self {
        Self {
            attribute: input.attribute,
            operator: pb::ConstraintOperator::from(input.operator) as i32,
            values: input
                .values
                .iter()
                .map(|v| json_to_prost_value(&v.0))
                .collect(),
        }
    }
}

#[derive(SimpleObject)]
#[graphql(name = "FeatureFlagsDistribution")]
pub struct Distribution {
    pub variant_key: String,
    pub weight: u32,
}

#[derive(InputObject)]
#[graphql(name = "FeatureFlagsDistributionInput")]
pub struct DistributionInput {
    pub variant_key: String,
    pub weight: u32,
}

#[derive(SimpleObject)]
#[graphql(name = "FeatureFlagsRule", complex)]
pub struct Rule {
    pub rank: u32,
    pub segment_key: Option<String>,
    pub variant_key: Option<String>,
    pub distributions: Vec<Distribution>,
    /// Groups of constraints the rule matches on.
    pub constraint_groups: Vec<Vec<Constraint>>,
    pub bucket_salt: String,
}

#[ComplexObject]
impl Rule {
    async fn segment(&self, ctx: &Context<'_>) -> Result<Option<Segment>> {
        match &self.segment_key {
            Some(key) => Segment::load(ctx, key).await.map(Some),
            None => Ok(None),
        }
    }
}

impl From<pb::Rule> for Rule {
    fn from(rule: pb::Rule) -> Self {
        Self {
            rank: rule.rank,
            segment_key: Some(rule.segment_key).filter(|k| !k.is_empty()),
            variant_key: Some(rule.variant_key).filter(|k| !k.is_empty()),
            distributions: rule
                .distributions
                .into_iter()
                .map(|d| Distribution {
                    variant_key: d.variant_key,
                    weight: d.weight,
                })
                .collect(),
            constraint_groups: rule
                .constraint_groups
                .into_iter()
                .map(|g| g.constraints.into_iter().map(Into::into).collect())
                .collect(),
            bucket_salt: rule.bucket_salt,
        }
    }
}

#[derive(InputObject)]
#[graphql(name = "FeatureFlagsRuleInput")]
pub struct RuleInput {
    pub rank: u32,
    pub segment_key: Option<String>,
    pub variant_key: Option<String>,
    #[graphql(default)]
    pub distributions: Vec<DistributionInput>,
    #[graphql(default)]
    pub constraint_groups: Vec<Vec<ConstraintInput>>,
    #[graphql(default)]
    pub bucket_salt: String,
}

impl From<RuleInput> for pb::Rule {
    fn from(input: RuleInput) -> Self {
        Self {
            rank: input.rank,
            segment_key: input.segment_key.unwrap_or_default(),
            variant_key: input.variant_key.unwrap_or_default(),
            distributions: input
                .distributions
                .into_iter()
                .map(|d| pb::Distribution {
                    variant_key: d.variant_key,
                    weight: d.weight,
                })
                .collect(),
            constraint_groups: input
                .constraint_groups
                .into_iter()
                .map(|g| pb::ConstraintGroup {
                    constraints: g.into_iter().map(Into::into).collect(),
                })
                .collect(),
            bucket_salt: input.bucket_salt,
        }
    }
}

#[derive(SimpleObject)]
#[graphql(name = "FeatureFlagsSegment", complex)]
pub struct Segment {
    pub key: String,
    pub name: String,
    pub constraints: Vec<Constraint>,
}

#[ComplexObject]
impl Segment {
    async fn id(&self) -> ID {
        IDS.encode("segment", &self.key)
    }

    async fn history(
        &self,
        ctx: &Context<'_>,
        #[graphql(default = 50)] limit: u32,
    ) -> Result<Vec<Change>> {
        Change::list(ctx, "segment", &self.key, limit).await
    }
}

impl From<pb::Segment> for Segment {
    fn from(segment: pb::Segment) -> Self {
        Self {
            key: segment.key,
            name: segment.name,
            constraints: segment.constraints.into_iter().map(Into::into).collect(),
        }
    }
}

impl Segment {
    pub async fn load(ctx: &Context<'_>, key: &str) -> Result<Self> {
        let services = ctx.data_unchecked::<Services>();
        let request = super::request(
            ctx,
            pb::GetSegmentRequest {
                key: key.to_owned(),
            },
        )?;
        let segment = Admin::get_segment(&*services.admin, request)
            .await
            .map_err(super::grpc)?;
        Ok(segment.into_inner().into())
    }
}

#[derive(InputObject)]
#[graphql(name = "FeatureFlagsSegmentInput")]
pub struct SegmentInput {
    pub key: String,
    pub name: String,
    #[graphql(default)]
    pub constraints: Vec<ConstraintInput>,
}

impl From<SegmentInput> for pb::Segment {
    fn from(input: SegmentInput) -> Self {
        Self {
            key: input.key,
            name: input.name,
            constraints: input.constraints.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(SimpleObject)]
#[graphql(name = "FeatureFlagsChange", complex)]
pub struct Change {
    #[graphql(skip)]
    pub uuid: String,
    pub version: i64,
    pub actor: String,
    pub action: String,
    pub target_kind: String,
    pub target_key: String,
    pub detail: Json<Value>,
    pub created_at: DateTime<Utc>,
}

#[ComplexObject]
impl Change {
    async fn id(&self) -> ID {
        IDS.encode("change", &self.uuid)
    }
}

impl From<crate::store::FlagChange> for Change {
    fn from(change: crate::store::FlagChange) -> Self {
        Self {
            uuid: change.id.to_string(),
            version: change.version,
            actor: change.actor,
            action: change.action,
            target_kind: change.target_kind,
            target_key: change.target_key,
            detail: Json(change.detail),
            created_at: change.created_at,
        }
    }
}

impl Change {
    pub async fn list(
        ctx: &Context<'_>,
        target_kind: &str,
        target_key: &str,
        limit: u32,
    ) -> Result<Vec<Self>> {
        let changes = ctx
            .data_unchecked::<Services>()
            .store
            .list_changes(target_kind, target_key, limit.into())
            .await
            .map_err(|e| super::grpc(e.into()))?;
        Ok(changes.into_iter().map(Into::into).collect())
    }
}

#[derive(SimpleObject)]
#[graphql(name = "FeatureFlagsEvaluation", complex)]
pub struct Evaluation {
    pub flag_key: String,
    pub value_type: ValueType,
    /// Null when the evaluation failed; see errorCode.
    pub value: Json<Value>,
    pub variant: Option<String>,
    pub reason: Reason,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
}

#[ComplexObject]
impl Evaluation {
    async fn flag(&self, ctx: &Context<'_>) -> Result<Flag> {
        Flag::load(ctx, &self.flag_key).await
    }
}

impl From<pb::EvaluatedFlag> for Evaluation {
    fn from(flag: pb::EvaluatedFlag) -> Self {
        let meta = flag.meta.unwrap_or_default();
        Self {
            value_type: value_type(flag.value_type),
            value: json(&flag.value),
            flag_key: flag.flag_key,
            variant: Some(meta.variant).filter(|v| !v.is_empty()),
            reason: pb::Reason::try_from(meta.reason)
                .unwrap_or(pb::Reason::Unspecified)
                .into(),
            error_code: Some(meta.error_code).filter(|v| !v.is_empty()),
            error_message: Some(meta.error_message).filter(|v| !v.is_empty()),
        }
    }
}

#[derive(InputObject)]
#[graphql(name = "FeatureFlagsContextInput")]
pub struct ContextInput {
    pub targeting_key: String,
    /// A JSON object of attributes rules can match on.
    pub attributes: Option<Json<Value>>,
}

impl From<ContextInput> for pb::EvaluationContext {
    fn from(input: ContextInput) -> Self {
        Self {
            targeting_key: input.targeting_key,
            attributes: input
                .attributes
                .map(|a| crate::convert::json_to_struct(&a.0)),
        }
    }
}

#[derive(SimpleObject)]
#[graphql(name = "FeatureFlagsConfigChange")]
pub struct ConfigChange {
    pub target_kind: String,
    pub target_key: String,
    pub op: ChangeOp,
    pub detail: Json<Value>,
}

impl From<pb::ConfigChange> for ConfigChange {
    fn from(change: pb::ConfigChange) -> Self {
        Self {
            op: pb::ChangeOp::try_from(change.op)
                .unwrap_or(pb::ChangeOp::Unspecified)
                .into(),
            detail: Json(serde_json::from_str(&change.detail).unwrap_or(Value::Null)),
            target_kind: change.target_kind,
            target_key: change.target_key,
        }
    }
}

#[derive(SimpleObject)]
#[graphql(name = "FeatureFlagsApplyResult")]
pub struct ApplyResult {
    pub changes: Vec<ConfigChange>,
    pub from_version: i64,
    pub to_version: i64,
    pub applied: bool,
}

#[derive(SimpleObject)]
#[graphql(name = "FeatureFlagsConfigEvent", complex)]
pub struct ConfigEvent {
    pub event_type: EventType,
    pub config_version: i64,
    pub changed_flag_keys: Vec<String>,
}

#[ComplexObject]
impl ConfigEvent {
    async fn changed_flags(&self, ctx: &Context<'_>) -> Result<Vec<Flag>> {
        let mut flags = Vec::with_capacity(self.changed_flag_keys.len());
        for key in &self.changed_flag_keys {
            match Flag::load(ctx, key).await {
                Ok(flag) => flags.push(flag),
                Err(_) => continue,
            }
        }
        Ok(flags)
    }
}
