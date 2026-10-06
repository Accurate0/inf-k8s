use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;
use std::collections::BTreeMap;

#[derive(Serialize, Deserialize, JsonSchema)]
#[schemars(
    description = "One flag file (`config/flags/<key>.yaml`). Variants are a `key: value` map; rules are ordered by position (their rank is assigned server-side on apply)."
)]
pub struct FlagDoc {
    pub key: String,
    #[serde(rename = "type")]
    pub value_type: String,
    #[serde(default)]
    pub enabled: bool,
    pub default: String,
    #[serde(default)]
    pub variants: BTreeMap<String, Json>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<RuleDoc>,
}

#[derive(Serialize, Deserialize, JsonSchema, Default)]
pub struct RuleDoc {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub segment: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub distributions: Vec<DistDoc>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(
        description = "Flat AND sugar: each constraint becomes its own single-element group."
    )]
    pub constraints: Vec<ConstraintDoc>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(
        description = "CNF: outer array AND-combined, inner arrays OR-combined. Takes precedence over `constraints` when present."
    )]
    pub constraint_groups: Vec<Vec<ConstraintDoc>>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub bucket_salt: String,
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct DistDoc {
    pub variant: String,
    pub weight: u32,
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct ConstraintDoc {
    pub attribute: String,
    pub operator: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub values: Vec<Json>,
}

#[derive(Serialize, Deserialize, JsonSchema, Default)]
#[schemars(description = "The `config/segments.yaml` file: all segments in one document.")]
pub struct SegmentsFile {
    #[serde(default)]
    pub segments: Vec<SegmentDoc>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct SegmentDoc {
    pub key: String,
    #[serde(default)]
    pub name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub constraints: Vec<ConstraintDoc>,
}
