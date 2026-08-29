use k8s_openapi::serde::{Deserialize, Serialize};
use kube::CustomResource;
use schemars::JsonSchema;

pub const DEFAULT_GATEWAY: &str = "public-gateway";

#[derive(CustomResource, Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[kube(
    kind = "WafBlock",
    group = "inf-k8s.net",
    version = "v1",
    namespaced,
    shortname = "wafb",
    category = "waf",
    status = "WafStatus",
    printcolumn = r#"{"name":"CIDR","type":"string","jsonPath":".spec.cidr"}"#,
    printcolumn = r#"{"name":"Gateway","type":"string","jsonPath":".spec.gateway"}"#,
    printcolumn = r#"{"name":"Accepted","type":"string","jsonPath":".status.conditions[?(@.type==\"Accepted\")].status"}"#,
    printcolumn = r#"{"name":"Expires","type":"string","jsonPath":".spec.expiresAt"}"#,
    printcolumn = r#"{"name":"By","type":"string","jsonPath":".spec.createdBy"}"#,
    printcolumn = r#"{"name":"Age","type":"date","jsonPath":".metadata.creationTimestamp"}"#
)]
pub struct WafBlockSpec {
    pub cidr: String,

    #[serde(default = "default_gateway")]
    pub gateway: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule_ids: Option<Vec<String>>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_by: Option<String>,
}

#[derive(CustomResource, Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[kube(
    kind = "WafPolicy",
    group = "inf-k8s.net",
    version = "v1",
    namespaced,
    shortname = "wafp",
    category = "waf",
    status = "WafStatus",
    printcolumn = r#"{"name":"Gateway","type":"string","jsonPath":".spec.gateway"}"#,
    printcolumn = r#"{"name":"Priority","type":"integer","jsonPath":".spec.priority"}"#,
    printcolumn = r#"{"name":"Accepted","type":"string","jsonPath":".status.conditions[?(@.type==\"Accepted\")].status"}"#,
    printcolumn = r#"{"name":"Age","type":"date","jsonPath":".metadata.creationTimestamp"}"#
)]
pub struct WafPolicySpec {
    #[serde(default = "default_gateway")]
    pub gateway: String,

    #[serde(default = "default_gateway_namespace")]
    pub gateway_namespace: String,

    #[serde(default)]
    pub priority: i32,

    #[schemars(schema_with = "preserve_unknown_object")]
    #[serde(default)]
    pub template: serde_json::Value,
}

#[derive(Deserialize, Serialize, Clone, Debug, Default, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WafStatus {
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

impl Condition {
    pub fn new(
        existing: Option<&Vec<Condition>>,
        type_: &str,
        status: &str,
        reason: &str,
        message: impl Into<String>,
        observed_generation: i64,
    ) -> Self {
        let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let last_transition_time = existing
            .and_then(|conds| conds.iter().find(|c| c.type_ == type_))
            .filter(|c| c.status == status)
            .map(|c| c.last_transition_time.clone())
            .unwrap_or(now);

        Self {
            type_: type_.to_string(),
            status: status.to_string(),
            reason: reason.to_string(),
            message: message.into(),
            observed_generation,
            last_transition_time,
        }
    }
}

impl WafBlock {
    pub fn is_expired(&self, now: chrono::DateTime<chrono::Utc>) -> bool {
        let Some(raw) = self.spec.expires_at.as_deref() else {
            return false;
        };

        match chrono::DateTime::parse_from_rfc3339(raw) {
            Ok(at) => at.with_timezone(&chrono::Utc) <= now,
            Err(_) => false,
        }
    }

    pub fn resource_name(net: &ipnet::IpNet) -> String {
        let mut slug = String::new();
        let mut last_dash = true;

        for ch in net.to_string().chars() {
            if ch.is_ascii_alphanumeric() {
                slug.push(ch.to_ascii_lowercase());
                last_dash = false;
            } else if !last_dash {
                slug.push('-');
                last_dash = true;
            }
        }

        let slug = slug.trim_matches('-');
        format!("waf-block-{slug}")
    }
}

fn default_gateway() -> String {
    DEFAULT_GATEWAY.to_string()
}

fn default_gateway_namespace() -> String {
    "envoy-gateway-system".to_string()
}

fn preserve_unknown_object(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    serde_json::from_value(serde_json::json!({
        "type": "object",
        "x-kubernetes-preserve-unknown-fields": true
    }))
    .expect("static schema must deserialize")
}
