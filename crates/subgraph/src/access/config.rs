use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::external::ClaimPattern;

pub type Grants = BTreeMap<String, BTreeMap<String, Vec<ClaimPattern>>>;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AccessConfig {
    pub issuers: BTreeMap<String, IssuerConfig>,

    #[serde(default)]
    pub scopes: Grants,

    pub subgraphs: BTreeMap<String, SubgraphGate>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum IssuerConfig {
    Kanidm { base: String },
    External { issuer: String },
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SubgraphGate {
    #[serde(default)]
    pub scopes: Vec<String>,

    #[serde(flatten)]
    pub issuers: BTreeMap<String, Value>,
}
