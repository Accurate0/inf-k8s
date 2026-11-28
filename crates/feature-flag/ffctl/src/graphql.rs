use chrono::{DateTime, Utc};
use serde_json::Value as Json;

#[cynic::schema("graph")]
mod schema {}

cynic::impl_scalar!(DateTime<Utc>, schema::DateTime);
cynic::impl_scalar!(Json, schema::JSON);

#[derive(cynic::Enum, Clone, Copy, Debug)]
#[cynic(graphql_type = "FeatureFlagsValueType")]
pub enum ValueType {
    Unspecified,
    Boolean,
    String,
    Integer,
    Float,
    Object,
}

#[derive(cynic::Enum, Clone, Copy, Debug)]
#[cynic(graphql_type = "FeatureFlagsConstraintOperator")]
pub enum Operator {
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

#[derive(cynic::Enum, Clone, Copy, Debug)]
#[cynic(graphql_type = "FeatureFlagsReason")]
pub enum Reason {
    Unspecified,
    Static,
    Default,
    TargetingMatch,
    Split,
    Disabled,
    Error,
}

#[derive(cynic::Enum, Clone, Copy, Debug)]
#[cynic(graphql_type = "FeatureFlagsChangeOp")]
pub enum ChangeOp {
    Unspecified,
    Create,
    Update,
    Delete,
}

#[derive(cynic::Enum, Clone, Copy, Debug)]
#[cynic(graphql_type = "FeatureFlagsEventType")]
pub enum EventType {
    Unspecified,
    Ready,
    ConfigurationChanged,
    Resync,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "FeatureFlagsVariant")]
pub struct Variant {
    pub key: String,
    pub value: Json,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "FeatureFlagsConstraint")]
pub struct Constraint {
    pub attribute: String,
    pub operator: Operator,
    pub values: Vec<Json>,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "FeatureFlagsDistribution")]
pub struct Distribution {
    pub variant_key: String,
    pub weight: i32,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "FeatureFlagsRule")]
pub struct Rule {
    pub rank: i32,
    pub segment_key: Option<String>,
    pub variant_key: Option<String>,
    pub distributions: Vec<Distribution>,
    pub constraint_groups: Vec<Vec<Constraint>>,
    pub bucket_salt: String,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "FeatureFlagsFlag")]
pub struct Flag {
    pub key: String,
    pub value_type: ValueType,
    pub enabled: bool,
    pub default_variant_key: String,
    pub archived: bool,
    pub variants: Vec<Variant>,
    pub rules: Vec<Rule>,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "FeatureFlagsSegment")]
pub struct Segment {
    pub key: String,
    pub name: String,
    pub constraints: Vec<Constraint>,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "FeatureFlagsChange")]
pub struct Change {
    pub id: cynic::Id,
    pub version: i32,
    pub actor: String,
    pub action: String,
    pub target_kind: String,
    pub target_key: String,
    pub detail: Json,
    pub created_at: DateTime<Utc>,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "FeatureFlagsConfigChange")]
pub struct ConfigChange {
    pub target_kind: String,
    pub target_key: String,
    pub op: ChangeOp,
    pub detail: Json,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "FeatureFlagsApplyResult")]
pub struct ApplyResult {
    pub changes: Vec<ConfigChange>,
    pub from_version: i32,
    pub to_version: i32,
    pub applied: bool,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "FeatureFlagsEvaluation")]
pub struct Evaluation {
    pub flag_key: String,
    pub value_type: ValueType,
    pub value: Json,
    pub variant: Option<String>,
    pub reason: Reason,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "FeatureFlagsConfigEvent")]
pub struct ConfigEvent {
    pub event_type: EventType,
    pub config_version: i32,
}

#[derive(cynic::InputObject, Debug)]
#[cynic(graphql_type = "FeatureFlagsVariantInput")]
pub struct VariantInput {
    pub key: String,
    pub value: Json,
}

#[derive(cynic::InputObject, Debug)]
#[cynic(graphql_type = "FeatureFlagsConstraintInput")]
pub struct ConstraintInput {
    pub attribute: String,
    pub operator: Operator,
    pub values: Vec<Json>,
}

#[derive(cynic::InputObject, Debug)]
#[cynic(graphql_type = "FeatureFlagsDistributionInput")]
pub struct DistributionInput {
    pub variant_key: String,
    pub weight: i32,
}

#[derive(cynic::InputObject, Debug)]
#[cynic(graphql_type = "FeatureFlagsRuleInput")]
pub struct RuleInput {
    pub rank: i32,
    pub segment_key: Option<String>,
    pub variant_key: Option<String>,
    pub distributions: Vec<DistributionInput>,
    pub constraint_groups: Vec<Vec<ConstraintInput>>,
    pub bucket_salt: String,
}

#[derive(cynic::InputObject, Debug)]
#[cynic(graphql_type = "FeatureFlagsCreateFlagInput")]
pub struct CreateFlagInput {
    pub key: String,
    pub value_type: ValueType,
    pub enabled: bool,
    pub default_variant_key: String,
    pub variants: Vec<VariantInput>,
}

#[derive(cynic::InputObject, Debug)]
#[cynic(graphql_type = "FeatureFlagsFlagInput")]
pub struct FlagInput {
    pub key: String,
    pub value_type: ValueType,
    pub enabled: bool,
    pub default_variant_key: String,
    pub archived: bool,
    pub variants: Vec<VariantInput>,
    pub rules: Vec<RuleInput>,
}

#[derive(cynic::InputObject, Debug)]
#[cynic(graphql_type = "FeatureFlagsSegmentInput")]
pub struct SegmentInput {
    pub key: String,
    pub name: String,
    pub constraints: Vec<ConstraintInput>,
}

#[derive(cynic::InputObject, Debug)]
#[cynic(graphql_type = "FeatureFlagsContextInput")]
pub struct ContextInput {
    pub targeting_key: String,
    pub attributes: Option<Json>,
}

#[derive(cynic::QueryVariables, Debug)]
pub struct FlagsVariables {
    pub include_archived: bool,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Query", variables = "FlagsVariables")]
pub struct FlagsQuery {
    #[arguments(includeArchived: $include_archived)]
    pub feature_flags: Option<Vec<Flag>>,
}

#[derive(cynic::QueryVariables, Debug)]
pub struct KeyVariables {
    pub key: String,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Query", variables = "KeyVariables")]
pub struct FlagQuery {
    #[arguments(key: $key)]
    pub feature_flag: Option<Flag>,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Query")]
pub struct SegmentsQuery {
    pub feature_flags_segments: Option<Vec<Segment>>,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Query", variables = "KeyVariables")]
pub struct SegmentQuery {
    #[arguments(key: $key)]
    pub feature_flags_segment: Option<Segment>,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Query")]
pub struct SnapshotQuery {
    pub feature_flags_config_version: Option<i32>,
    #[arguments(includeArchived: false)]
    pub feature_flags: Option<Vec<Flag>>,
    pub feature_flags_segments: Option<Vec<Segment>>,
}

#[derive(cynic::QueryVariables, Debug)]
pub struct HistoryVariables {
    pub target_kind: Option<String>,
    pub target_key: Option<String>,
    pub limit: i32,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Query", variables = "HistoryVariables")]
pub struct HistoryQuery {
    #[arguments(targetKind: $target_kind, targetKey: $target_key, limit: $limit)]
    pub feature_flags_history: Option<Vec<Change>>,
}

#[derive(cynic::QueryVariables, Debug)]
pub struct EvaluateAllVariables {
    pub context: ContextInput,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Query", variables = "EvaluateAllVariables")]
pub struct EvaluateAllQuery {
    #[arguments(context: $context)]
    pub feature_flags_evaluate_all: Option<Vec<Evaluation>>,
}

#[derive(cynic::QueryVariables, Debug)]
pub struct EvaluateVariables {
    pub flag_key: String,
    pub context: ContextInput,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Query", variables = "EvaluateVariables")]
pub struct EvaluateQuery {
    #[arguments(flagKey: $flag_key, context: $context)]
    pub feature_flags_evaluate: Option<Evaluation>,
}

#[derive(cynic::QueryVariables, Debug)]
pub struct CreateFlagVariables {
    pub input: CreateFlagInput,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Mutation", variables = "CreateFlagVariables")]
pub struct CreateFlag {
    #[arguments(input: $input)]
    pub feature_flags_create_flag: Flag,
}

#[derive(cynic::QueryVariables, Debug)]
pub struct UpdateFlagVariables {
    pub key: String,
    pub enabled: bool,
    pub default_variant_key: String,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Mutation", variables = "UpdateFlagVariables")]
pub struct UpdateFlag {
    #[arguments(key: $key, enabled: $enabled, defaultVariantKey: $default_variant_key)]
    pub feature_flags_update_flag: Flag,
}

#[derive(cynic::QueryVariables, Debug)]
pub struct ArchiveFlagVariables {
    pub key: String,
    pub archived: bool,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Mutation", variables = "ArchiveFlagVariables")]
pub struct ArchiveFlag {
    #[arguments(key: $key, archived: $archived)]
    pub feature_flags_archive_flag: Flag,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Mutation", variables = "KeyVariables")]
pub struct DeleteFlag {
    #[arguments(key: $key)]
    pub feature_flags_delete_flag: cynic::Id,
}

#[derive(cynic::QueryVariables, Debug)]
pub struct UpsertVariantVariables {
    pub flag_key: String,
    pub variant: VariantInput,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Mutation", variables = "UpsertVariantVariables")]
pub struct UpsertVariant {
    #[arguments(flagKey: $flag_key, variant: $variant)]
    pub feature_flags_upsert_variant: Flag,
}

#[derive(cynic::QueryVariables, Debug)]
pub struct DeleteVariantVariables {
    pub flag_key: String,
    pub variant_key: String,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Mutation", variables = "DeleteVariantVariables")]
pub struct DeleteVariant {
    #[arguments(flagKey: $flag_key, variantKey: $variant_key)]
    pub feature_flags_delete_variant: Flag,
}

#[derive(cynic::QueryVariables, Debug)]
pub struct SetRulesVariables {
    pub flag_key: String,
    pub rules: Vec<RuleInput>,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Mutation", variables = "SetRulesVariables")]
pub struct SetFlagRules {
    #[arguments(flagKey: $flag_key, rules: $rules)]
    pub feature_flags_set_flag_rules: Flag,
}

#[derive(cynic::QueryVariables, Debug)]
pub struct SegmentVariables {
    pub input: SegmentInput,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Mutation", variables = "SegmentVariables")]
pub struct UpdateSegment {
    #[arguments(input: $input)]
    pub feature_flags_update_segment: Segment,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Mutation", variables = "KeyVariables")]
pub struct DeleteSegment {
    #[arguments(key: $key)]
    pub feature_flags_delete_segment: cynic::Id,
}

#[derive(cynic::QueryVariables, Debug)]
pub struct ApplyConfigVariables {
    pub flags: Vec<FlagInput>,
    pub segments: Vec<SegmentInput>,
    pub dry_run: bool,
    pub expected_version: i32,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Mutation", variables = "ApplyConfigVariables")]
pub struct ApplyConfig {
    #[arguments(flags: $flags, segments: $segments, dryRun: $dry_run, expectedVersion: $expected_version)]
    pub feature_flags_apply_config: ApplyResult,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Subscription")]
pub struct ChangesSubscription {
    pub feature_flags_changes: ConfigEvent,
}
