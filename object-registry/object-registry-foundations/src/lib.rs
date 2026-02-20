pub mod audit_manager;
pub mod event_manager;
pub mod key_manager;
pub mod object_manager;

use jsonwebtoken::Algorithm;
use jsonwebtoken::EncodingKey;
use jsonwebtoken::Header;
use object_registry::ObjectRegistryJwtClaims;
use object_registry::Role;
use thiserror::Error;

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
