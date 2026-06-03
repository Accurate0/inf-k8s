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
    /// Claim name -> group-membership-derived claim values. Lets an app
    /// auto-promote members of a kanidm group (e.g. expose a "forgejo" claim
    /// containing "admin" for platform_admins so Forgejo grants them admin).
    #[serde(default)]
    pub claim_maps: BTreeMap<String, ClaimMap>,
    /// Secret to write (clientId/clientSecret/issuerUrl by default).
    pub secret_name: String,
    pub secret_namespace: String,
    /// Override the key names used in the written Secret (e.g. Forgejo wants key/secret).
    #[serde(default)]
    pub secret_keys: SecretKeys,
    /// Extra labels to set on the written Secret (e.g. Argo CD needs
    /// app.kubernetes.io/part-of: argocd to read it).
    #[serde(default)]
    pub secret_labels: BTreeMap<String, String>,
    /// Public (PKCE-only, no secret) client. Defaults to false (confidential).
    #[serde(default)]
    pub public: bool,
    /// Disable the PKCE requirement for confidential clients that don't support it.
    #[serde(default)]
    pub allow_insecure_client_disable_pkce: bool,
    /// Sign tokens with RS256 instead of the default ES256. Some OIDC clients
    /// (e.g. Flipt's go-oidc) only accept RS256.
    #[serde(default)]
    pub enable_legacy_crypto: bool,
    /// Expose the short username (instead of the SPN) as preferred_username.
    #[serde(default)]
    pub prefer_short_username: bool,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ClaimMap {
    /// How a member's values are joined in the emitted claim.
    #[serde(default)]
    pub join: ClaimMapJoin,
    /// Group name -> claim values contributed by that group's members.
    pub values_by_group: BTreeMap<String, Vec<String>>,
}

#[derive(Deserialize, Serialize, Clone, Copy, Debug, JsonSchema, Default)]
#[serde(rename_all = "lowercase")]
pub enum ClaimMapJoin {
    #[default]
    Array,
    Csv,
    Ssv,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SecretKeys {
    #[serde(default = "default_client_id_key")]
    pub client_id: String,
    #[serde(default = "default_client_secret_key")]
    pub client_secret: String,
    #[serde(default = "default_issuer_url_key")]
    pub issuer_url: String,
}

impl Default for SecretKeys {
    fn default() -> Self {
        Self {
            client_id: default_client_id_key(),
            client_secret: default_client_secret_key(),
            issuer_url: default_issuer_url_key(),
        }
    }
}

fn default_client_id_key() -> String {
    "clientId".to_string()
}
fn default_client_secret_key() -> String {
    "clientSecret".to_string()
}
fn default_issuer_url_key() -> String {
    "issuerUrl".to_string()
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct KanidmOAuth2ClientStatus {
    pub provisioned: bool,
}
