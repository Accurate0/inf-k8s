use jsonwebtoken::Algorithm;
use jsonwebtoken::DecodingKey;
use jsonwebtoken::EncodingKey;
use jsonwebtoken::Header;
use jsonwebtoken::Validation;
use serde::Deserialize;
use serde::Serialize;
use thiserror::Error;

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
    #[error("Token header is invalid")]
    InvalidTokenHeader,
    #[error("No matching JWK was found")]
    NoMatchingJwk,
    #[error("No matching repository was found")]
    NoMatchingRepository,
}

pub fn verify_jwt(
    secret: &[u8],
    unverified_token: &str,
) -> Result<ObjectRegistryJwtClaims, JwtValidationError> {
    let validation = {
        let mut validation = Validation::new(Algorithm::default());
        validation.set_issuer(&["home-gateway", "config-catalog-cli", "config-catalog"]);
        validation.set_audience(&["config-catalog", "home-gateway"]);
        validation.validate_exp = true;
        validation
    };

    let validated_token = jsonwebtoken::decode::<ObjectRegistryJwtClaims>(
        unverified_token.as_bytes(),
        &DecodingKey::from_secret(secret),
        &validation,
    )?;

    Ok(validated_token.claims)
}

pub fn generate_jwt(
    secret: &[u8],
    creator: &str,
    created_for: &str,
) -> Result<String, JwtValidationError> {
    let now = chrono::offset::Utc::now().timestamp();
    let claims = ObjectRegistryJwtClaims {
        iat: now,
        // 15mins
        exp: now + 900,
        aud: created_for.to_owned(),
        iss: creator.to_owned(),
        sub: created_for.to_owned(),
        role: vec![Role::Service],
    };

    let token = jsonwebtoken::encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret),
    )?;

    Ok(token)
}
