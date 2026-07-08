//! The declarative YAML config schema for `ffctl config` (Terraform-style flag
//! management). These are the on-disk document types; conversion to and from the
//! wire types lives in the `ffctl` binary. The JSON Schemas under `config/schema`
//! are generated from these types by the `gen-schema` binary — edit the types, then
//! regenerate; don't hand-edit the `.json`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;
use std::collections::BTreeMap;

/// One flag file (`config/flags/<key>.yaml`). Variants are a `key: value` map; rules
/// are ordered by position (their rank is assigned server-side on apply).
#[derive(Serialize, Deserialize, JsonSchema)]
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
    /// Flat AND sugar: each constraint becomes its own single-element group.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub constraints: Vec<ConstraintDoc>,
    /// CNF: outer array AND-combined, inner arrays OR-combined. Takes precedence over
    /// `constraints` when present.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
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

/// The `config/segments.yaml` file: all segments in one document.
#[derive(Serialize, Deserialize, JsonSchema, Default)]
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
