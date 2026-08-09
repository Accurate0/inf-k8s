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
    /// The database this resource owns. It is created if missing, and its owner is
    /// set to this resource's role on every reconcile - so two PostgresDatabase
    /// resources must never name the same database here, or they will flap
    /// ownership between their roles. To share an existing database, list it under
    /// additionalDatabases instead.
    #[schemars(regex(pattern = r"^[A-Za-z_][A-Za-z0-9_]*$"))]
    pub database_name: String,

    #[schemars(regex(pattern = r"^[A-Za-z_][A-Za-z0-9_]*$"))]
    pub role_name: Option<String>,

    /// Existing databases this role should also get access to. The role is granted
    /// ALL PRIVILEGES on each one plus membership in that database's owning role, so
    /// it can act with owner rights via SET ROLE. Ownership itself is never
    /// reassigned - a Postgres database has exactly one owner, and it stays with
    /// whichever role already holds it. Databases that do not exist are skipped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(inner(regex(pattern = r"^[A-Za-z_][A-Za-z0-9_]*$")))]
    pub additional_databases: Option<Vec<String>>,

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
