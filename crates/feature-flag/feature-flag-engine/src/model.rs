//! In-memory domain model the evaluation engine operates on. These are decoupled
//! from both the generated protobuf types and the database rows; conversions live
//! in [`crate::convert`] and [`crate::store`].

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ValueType {
    Boolean,
    String,
    Integer,
    Float,
    Object,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Variant {
    pub key: String,
    pub value: serde_json::Value,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Operator {
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
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Constraint {
    pub attribute: String,
    pub operator: Operator,
    pub values: Vec<serde_json::Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Segment {
    pub key: String,
    pub name: String,
    pub constraints: Vec<Constraint>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Distribution {
    pub variant_key: String,
    pub weight: u32,
}

/// An ordered targeting rule. `segment_key == None` matches every context (a
/// catch-all rollout); otherwise the named segment must match.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Rule {
    pub rank: u32,
    pub segment_key: Option<String>,
    pub variant_key: Option<String>,
    pub distributions: Vec<Distribution>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Flag {
    pub key: String,
    pub value_type: ValueType,
    pub enabled: bool,
    pub default_variant_key: String,
    pub archived: bool,
    pub variants: Vec<Variant>,
    pub rules: Vec<Rule>,
}

impl Flag {
    pub fn variant(&self, key: &str) -> Option<&Variant> {
        self.variants.iter().find(|v| v.key == key)
    }
}

/// An immutable, fully-resolved view of every flag and segment at a given config
/// version. The engine reads exclusively from a snapshot so evaluation never
/// touches the database.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Snapshot {
    pub version: i64,
    pub flags: HashMap<String, Flag>,
    pub segments: HashMap<String, Segment>,
}
