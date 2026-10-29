pub mod oidc;
pub mod policy;
pub mod tokens;

use axum::{extract::FromRequestParts, http::request::Parts};
use registry_api::TOKEN_PREFIX;
use serde::{Deserialize, Deserializer};
use sqlx::PgPool;

use crate::{config::Config, error::ApiError, state::AppState};
use oidc::{Verifier, VerifyError, groups_match, unverified_issuer};
use tokens::TokenPrincipal;

#[derive(Debug, Clone, Deserialize)]
pub struct UserClaims {
    pub sub: String,
    #[serde(default)]
    pub preferred_username: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub groups: Option<Vec<String>>,
}

impl UserClaims {
    pub fn identity(&self) -> &str {
        self.preferred_username
            .as_deref()
            .or(self.email.as_deref())
            .unwrap_or(&self.sub)
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ForgejoClaims {
    pub repository: String,
    #[serde(default, deserialize_with = "stringish")]
    pub repository_id: Option<String>,
    #[serde(default, rename = "ref")]
    pub git_ref: Option<String>,
    #[serde(default)]
    pub sha: Option<String>,
    #[serde(default, deserialize_with = "stringish")]
    pub run_id: Option<String>,
    #[serde(default)]
    pub workflow_ref: Option<String>,
    #[serde(default)]
    pub event_name: Option<String>,
    #[serde(default)]
    pub actor: Option<String>,
}

fn stringish<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<String>, D::Error> {
    Ok(
        match Option::<serde_json::Value>::deserialize(deserializer)? {
            Some(serde_json::Value::String(s)) => Some(s),
            Some(serde_json::Value::Number(n)) => Some(n.to_string()),
            _ => None,
        },
    )
}

#[derive(Debug, Clone)]
pub enum Principal {
    User {
        name: String,
        groups: Vec<String>,
        admin: bool,
        reader: bool,
    },
    Token(TokenPrincipal),
    Forgejo(ForgejoClaims),
}

impl Principal {
    pub fn actor(&self) -> String {
        match self {
            Principal::User { name, .. } => name.clone(),
            Principal::Token(token) => format!("token:{}", token.name),
            Principal::Forgejo(claims) => format!("forgejo:{}", claims.repository),
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Principal::User { .. } => "user",
            Principal::Token(_) => "token",
            Principal::Forgejo(_) => "forgejo",
        }
    }

    pub fn is_admin(&self) -> bool {
        matches!(self, Principal::User { admin: true, .. })
    }

    pub fn require_reader(&self) -> Result<(), ApiError> {
        match self {
            Principal::User {
                reader,
                admin,
                name,
                ..
            } if !reader && !admin => Err(ApiError::Forbidden(format!(
                "{name} is not in a registry read group"
            ))),
            _ => Ok(()),
        }
    }

    pub fn require_admin(&self) -> Result<(), ApiError> {
        if self.is_admin() {
            Ok(())
        } else {
            Err(ApiError::Forbidden(format!(
                "{} is not a registry admin",
                self.actor()
            )))
        }
    }
}

#[derive(Clone)]
pub struct Auth {
    kanidm: Verifier,
    userinfo_endpoint: String,
    read_groups: Vec<String>,
    admin_groups: Vec<String>,
    forgejo: Option<Verifier>,
}

impl Auth {
    pub fn from_config(config: &Config) -> Self {
        let oauth = &config.oauth;
        let base = oauth.issuer.trim_end_matches('/');

        let kanidm = Verifier::new(
            &oauth.issuer,
            oauth.audience.clone(),
            oauth
                .jwks_uri
                .clone()
                .unwrap_or_else(|| format!("{base}/public_key.jwk")),
        );

        let forgejo = config.forgejo.as_ref().map(|forgejo| {
            let base = forgejo.issuer.trim_end_matches('/');
            Verifier::new(
                &forgejo.issuer,
                Some(forgejo.audience.clone()),
                forgejo
                    .jwks_uri
                    .clone()
                    .unwrap_or_else(|| format!("{base}/.well-known/keys")),
            )
        });

        Self {
            kanidm,
            userinfo_endpoint: oauth
                .userinfo_endpoint
                .clone()
                .unwrap_or_else(|| format!("{base}/userinfo")),
            read_groups: oauth.read_groups.clone(),
            admin_groups: oauth.admin_groups.clone(),
            forgejo,
        }
    }

    pub fn forgejo_issuer(&self) -> Option<&str> {
        self.forgejo.as_ref().map(Verifier::issuer)
    }

    pub async fn start(&self) {
        for verifier in std::iter::once(&self.kanidm).chain(self.forgejo.as_ref()) {
            if let Err(e) = verifier.refresh().await {
                tracing::warn!(
                    issuer = verifier.issuer(),
                    "initial jwks fetch failed, retrying on demand: {e}"
                );
            }
            tokio::spawn(verifier.clone().run());
        }
    }

    pub async fn authenticate(&self, pool: &PgPool, bearer: &str) -> Result<Principal, ApiError> {
        if bearer.starts_with(TOKEN_PREFIX) {
            return tokens::lookup(pool, bearer)
                .await?
                .map(Principal::Token)
                .ok_or_else(|| {
                    ApiError::Unauthorized("invalid, revoked or expired token".to_owned())
                });
        }

        let issuer = unverified_issuer(bearer).ok_or_else(|| {
            ApiError::Unauthorized("bearer is neither a token nor a JWT".to_owned())
        })?;

        if let Some(forgejo) = self.forgejo.as_ref().filter(|v| v.issuer() == issuer) {
            let claims: ForgejoClaims = forgejo.verify(bearer).await.map_err(verify_error)?;
            return Ok(Principal::Forgejo(claims));
        }

        if issuer != self.kanidm.issuer() {
            return Err(ApiError::Unauthorized(format!("untrusted issuer {issuer}")));
        }

        let claims: UserClaims = self.kanidm.verify(bearer).await.map_err(verify_error)?;
        let groups = match &claims.groups {
            Some(groups) => groups.clone(),
            None => self
                .kanidm
                .userinfo_groups(&self.userinfo_endpoint, bearer)
                .await
                .map_err(verify_error)?,
        };

        Ok(Principal::User {
            name: claims.identity().to_owned(),
            admin: groups_match(&groups, &self.admin_groups),
            reader: groups_match(&groups, &self.read_groups),
            groups,
        })
    }
}

fn verify_error(e: VerifyError) -> ApiError {
    match e {
        VerifyError::Rejected(reason) => ApiError::Unauthorized(reason),
        other => ApiError::AuthUnavailable(other.to_string()),
    }
}

impl FromRequestParts<AppState> for Principal {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let bearer = parts
            .headers
            .get(http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| {
                v.strip_prefix("Bearer ")
                    .or_else(|| v.strip_prefix("bearer "))
            })
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .ok_or_else(|| {
                ApiError::Unauthorized("missing bearer token; run `ortzi login`".to_owned())
            })?;

        state.auth.authenticate(&state.pool, bearer).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forgejo_ids_may_be_numbers_or_strings() {
        let claims: ForgejoClaims = serde_json::from_value(serde_json::json!({
            "repository": "anurag/k8s",
            "repository_id": 42,
            "run_id": "7",
            "ref": "refs/heads/main",
        }))
        .unwrap();

        assert_eq!(claims.repository_id.as_deref(), Some("42"));
        assert_eq!(claims.run_id.as_deref(), Some("7"));
        assert_eq!(claims.git_ref.as_deref(), Some("refs/heads/main"));
    }
}
