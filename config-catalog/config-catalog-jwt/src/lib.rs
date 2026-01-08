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
pub struct ConfigCatalogJwtClaims {
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

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubActionsClaims {
    pub jti: String,
    pub sub: String,
    pub environment: String,
    pub aud: String,
    #[serde(rename = "ref")]
    pub ref_field: String,
    pub sha: String,
    pub repository: String,
    #[serde(rename = "repository_owner")]
    pub repository_owner: String,
    #[serde(rename = "actor_id")]
    pub actor_id: String,
    #[serde(rename = "repository_visibility")]
    pub repository_visibility: String,
    #[serde(rename = "repository_id")]
    pub repository_id: String,
    #[serde(rename = "repository_owner_id")]
    pub repository_owner_id: String,
    #[serde(rename = "run_id")]
    pub run_id: String,
    #[serde(rename = "run_number")]
    pub run_number: String,
    #[serde(rename = "run_attempt")]
    pub run_attempt: String,
    #[serde(rename = "runner_environment")]
    pub runner_environment: String,
    pub actor: String,
    pub workflow: String,
    #[serde(rename = "head_ref")]
    pub head_ref: String,
    #[serde(rename = "base_ref")]
    pub base_ref: String,
    #[serde(rename = "event_name")]
    pub event_name: String,
    #[serde(rename = "ref_type")]
    pub ref_type: String,
    #[serde(rename = "job_workflow_ref")]
    pub job_workflow_ref: String,
    pub iss: String,
    pub nbf: i64,
    pub exp: i64,
    pub iat: i64,
}

pub fn verify_jwt(
    secret: &[u8],
    unverified_token: &str,
) -> Result<ConfigCatalogJwtClaims, JwtValidationError> {
    let validated_token = jsonwebtoken::decode::<ConfigCatalogJwtClaims>(
        unverified_token.as_bytes(),
        &DecodingKey::from_secret(secret),
        &Validation::default(),
    )?;

    Ok(validated_token.claims)
}

pub fn generate_jwt(
    secret: &[u8],
    creator: &str,
    created_for: &str,
) -> Result<String, JwtValidationError> {
    let now = chrono::offset::Utc::now().timestamp();
    let claims = ConfigCatalogJwtClaims {
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

pub fn verify_github_actions_token(
    jwks: JwkSet,
    token: &str,
    allowed_repos: &[&str],
) -> Result<GithubActionsClaims, JwtValidationError> {
    let header = jsonwebtoken::decode_header(token)?;

    let Some(kid) = header.kid else {
        return Err(JwtValidationError::InvalidTokenHeader);
    };

    let Some(jwk) = jwks.find(&kid) else {
        return Err(JwtValidationError::NoMatchingJwk);
    };

    let validation = {
        let mut validation = Validation::new(header.alg);
        validation.set_issuer(&["https://token.actions.githubusercontent.com"]);
        validation.validate_exp = true;
        validation
    };

    let decoded_token =
        jsonwebtoken::decode::<GithubActionsClaims>(token, &jwk.try_into()?, &validation)?;

    if !allowed_repos.contains(&decoded_token.claims.repository.as_str()) {
        return Err(JwtValidationError::NoMatchingRepository);
    }

    Ok(decoded_token.claims)
}
