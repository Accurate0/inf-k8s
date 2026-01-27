use jsonwebtoken::Algorithm;
use jsonwebtoken::EncodingKey;
use jsonwebtoken::Header;
use serde::Deserialize;
use serde::Serialize;
use thiserror::Error;

pub mod event_manager;
pub mod key_manager;
pub mod object_manager;
pub mod types;
pub use types::{CreatedResponse, EventRequest, EventResponse, NotifyRequest, NotifyResponse};
pub mod api_client;
pub use api_client::ApiClient;

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

#[derive(Error, Debug)]
pub enum JwtValidationError {
    #[error(r#"JwtError has occurred: `{0}`"#)]
    JwtError(#[from] jsonwebtoken::errors::Error),
}

pub fn generate_jwt_from_private_key(
    secret: &[u8],
    creator: &str,
    created_for: &str,
) -> Result<String, JwtValidationError> {
    let now = chrono::offset::Utc::now().timestamp();
    let claims = ObjectRegistryJwtClaims {
        iat: now,
        exp: now + 900,
        aud: created_for.to_owned(),
        iss: creator.to_owned(),
        sub: created_for.to_owned(),
        role: vec![Role::Service],
    };

    let token = jsonwebtoken::encode(
        &Header::new(Algorithm::RS256),
        &claims,
        &EncodingKey::from_rsa_pem(secret)?,
    )?;

    Ok(token)
}


