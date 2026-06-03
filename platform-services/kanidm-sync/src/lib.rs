use k8s_openapi::serde::{Deserialize, Serialize};
use kube::CustomResource;
use schemars::JsonSchema;
use std::collections::BTreeMap;

#[derive(CustomResource, Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[kube(
    kind = "KanidmOAuth2Client",
    group = "inf-k8s.net",
    version = "v1",
    namespaced
)]
#[kube(status = "KanidmOAuth2ClientStatus")]
pub struct KanidmOAuth2ClientSpec {
    /// The kanidm resource server name (lowercase, e.g. "forgejo").
    pub name: String,
    pub display_name: String,
    /// Landing/origin URL (oauth2_rs_origin_landing).
    pub origin: String,
    /// Exact redirect/callback URLs (oauth2_rs_origin). Strict matching is enforced.
    pub redirect_urls: Vec<String>,
    /// Group name -> OIDC scopes granted to its members.
    #[serde(default)]
    pub scope_maps: BTreeMap<String, Vec<String>>,
    /// Canonical Secret to write (clientId/clientSecret/issuerUrl).
    pub secret_name: String,
    pub secret_namespace: String,
    /// Public (PKCE-only, no secret) client. Defaults to false (confidential).
    #[serde(default)]
    pub public: bool,
    /// Disable the PKCE requirement for confidential clients that don't support it.
    #[serde(default)]
    pub allow_insecure_client_disable_pkce: bool,
    /// Expose the short username (instead of the SPN) as preferred_username.
    #[serde(default)]
    pub prefer_short_username: bool,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct KanidmOAuth2ClientStatus {
    pub provisioned: bool,
}
