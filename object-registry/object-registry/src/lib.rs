use serde::Deserialize;
use serde::Serialize;

pub mod types;
pub use types::{
    CreatedResponse, EventRequest, EventResponse, MetadataResponse, NotifyRequest, NotifyResponse,
    ObjectResponse, OptionalObjectResponse,
};
pub mod api_client;
pub use api_client::ApiClient;

pub const X_AUDIT_ID_HEADER: &str = "x-object-registry-audit-id";

pub use jsonwebtoken::jwk::JwkSet;

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Role {
    #[default]
    Service,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ObjectRegistryJwtClaims {
    pub iat: i64,
    pub exp: i64,
    pub aud: String,
    pub iss: String,
    pub sub: String,
    #[serde(default)]
    pub role: Vec<Role>,
}
