use hmac::Hmac;
use hmac::Mac;
use jwt::Header;
use jwt::SignWithKey;
use jwt::Token;
use jwt::VerifyWithKey;
use serde::Deserialize;
use serde::Serialize;
use sha2::Sha256;
use sha2::digest::InvalidLength;
use thiserror::Error;

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Role {
    #[default]
    Service,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JwtClaims {
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
    #[error("Key length error has occurred: `{0}`")]
    InvalidKeyLength(#[from] InvalidLength),
    #[error(r#"JwtError has occurred: `{0}`"#)]
    JwtError(#[from] jwt::Error),
    #[error("Jwt is expired")]
    Expired,
}

pub fn verify_jwt(secret: &[u8], unverified_token: &str) -> Result<JwtClaims, JwtValidationError> {
    let key: Hmac<Sha256> = Hmac::new_from_slice(secret)?;
    let unverified: Token<Header, JwtClaims, jwt::Unverified<'_>> =
        Token::parse_unverified(unverified_token)?;

    let token: Token<_, _, jwt::Verified> = unverified.verify_with_key(&key)?;
    let now = chrono::offset::Utc::now().timestamp();
    if now > token.claims().exp {
        return Err(JwtValidationError::Expired);
    }

    Ok(token.claims().clone())
}

pub fn generate_jwt(
    secret: &[u8],
    creator: &str,
    created_for: &str,
) -> Result<String, JwtValidationError> {
    let key: Hmac<Sha256> = Hmac::new_from_slice(secret)?;

    let now = chrono::offset::Utc::now().timestamp();
    let claims = JwtClaims {
        iat: now,
        // 15mins
        exp: now + 900,
        aud: created_for.to_owned(),
        iss: creator.to_owned(),
        sub: created_for.to_owned(),
        role: vec![Role::Service],
    };

    Ok(claims.sign_with_key(&key)?)
}
