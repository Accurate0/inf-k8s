use k8s_openapi::serde::{Deserialize, Serialize};
use kube::CustomResource;
use schemars::JsonSchema;

pub const IDENT_PATTERN: &str = r"^[A-Za-z_][A-Za-z0-9_]*$";

#[derive(CustomResource, Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[kube(
    kind = "PostgresDatabase",
    group = "inf-k8s.net",
    version = "v1",
    namespaced,
    shortname = "pgdb",
    category = "databases",
    status = "PostgresDatabaseStatus",
    printcolumn = r#"{"name":"Database","type":"string","jsonPath":".spec.databaseName"}"#,
    printcolumn = r#"{"name":"Ready","type":"string","jsonPath":".status.conditions[?(@.type==\"Programmed\")].status"}"#,
    printcolumn = r#"{"name":"Age","type":"date","jsonPath":".metadata.creationTimestamp"}"#
)]
pub struct PgDatabaseSpec {
    #[schemars(regex(pattern = r"^[A-Za-z_][A-Za-z0-9_]*$"))]
    pub database_name: String,

    #[schemars(regex(pattern = r"^[A-Za-z_][A-Za-z0-9_]*$"))]
    pub role_name: Option<String>,

    pub secret_name: String,

    pub secret_namespace: String,
}

#[derive(Deserialize, Serialize, Clone, Debug, Default, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PostgresDatabaseStatus {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conditions: Vec<Condition>,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Condition {
    #[serde(rename = "type")]
    pub type_: String,
    pub status: String,
    pub reason: String,
    pub message: String,
    pub observed_generation: i64,
    pub last_transition_time: String,
}
