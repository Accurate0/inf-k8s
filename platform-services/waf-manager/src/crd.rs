use k8s_openapi::serde::{Deserialize, Serialize};
use kube::CustomResource;
use schemars::JsonSchema;

pub const DEFAULT_GATEWAY: &str = "public-gateway";

/// A client CIDR denied at a gateway. Every WafBlock for a gateway is folded
/// into one Deny rule in the compiled SecurityPolicy.
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
    /// A bare address is widened to /32 or /128. Blocks overlapping the tailnet,
    /// private ranges, or Cloudflare's ranges are rejected.
    pub cidr: String,

    /// Must match a WafPolicy's gateway for the block to have anywhere to land.
    #[serde(default = "default_gateway")]
    pub gateway: String,

    /// Free text, not interpreted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,

    /// Coraza rule ids that motivated the block, for context only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule_ids: Option<Vec<String>>,

    /// RFC 3339. Once passed the controller deletes this resource. Omit for a
    /// permanent block.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,

    /// Who created the block, from the OIDC `preferred_username` claim that Envoy
    /// injects. Empty for blocks applied from git.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_by: Option<String>,
}

/// A contribution to the SecurityPolicy waf-manager compiles for a gateway.
///
/// Envoy Gateway allows only one SecurityPolicy per Gateway and
/// `authorization.rules` is an atomic list, so the real resource cannot be
/// co-edited. waf-manager owns it and merges these templates instead.
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
    /// Gateway in `gatewayNamespace` whose SecurityPolicy this contributes to.
    #[serde(default = "default_gateway")]
    pub gateway: String,

    #[serde(default = "default_gateway_namespace")]
    pub gateway_namespace: String,

    /// Lower merges first. Envoy evaluates authorization rules first-match-wins,
    /// so order matters. Generated block rules always precede template rules.
    #[serde(default)]
    pub priority: i32,

    /// A SecurityPolicy `spec`. Setting `targetRefs`, `targetRef`,
    /// `targetSelectors` or `mergeType` is reported as a conflict. Single-valued
    /// blocks (`oidc`, `jwt`, `basicAuth`, `cors`, `extAuth`, `apiKeyAuth`) may be
    /// set by one WafPolicy per gateway; `authorization.rules` are concatenated.
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
    /// Carries `last_transition_time` forward when the status is unchanged.
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
    /// An unparsable timestamp counts as not expired; refusing to delete is the
    /// safe failure.
    pub fn is_expired(&self, now: chrono::DateTime<chrono::Utc>) -> bool {
        let Some(raw) = self.spec.expires_at.as_deref() else {
            return false;
        };

        match chrono::DateTime::parse_from_rfc3339(raw) {
            Ok(at) => at.with_timezone(&chrono::Utc) <= now,
            Err(_) => false,
        }
    }

    /// Derived from the CIDR so a double submit, or a workflow racing an
    /// operator on the same address, collides rather than duplicating.
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

/// The SecurityPolicy schema lives in another CRD, so the template is stored
/// opaquely and validated when the compiled policy is applied.
fn preserve_unknown_object(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    serde_json::from_value(serde_json::json!({
        "type": "object",
        "x-kubernetes-preserve-unknown-fields": true
    }))
    .expect("static schema must deserialize")
}
