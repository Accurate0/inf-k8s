use crate::config::OAuthConfig;
use jsonwebtoken::jwk::{AlgorithmParameters, Jwk, JwkSet};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("unauthorized: {0}")]
    Unauthorized(String),

    #[error("forbidden: {0}")]
    Forbidden(String),

    #[error("jwks error: {0}")]
    Jwks(String),

    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
}

#[derive(Debug, Clone, Deserialize)]
pub struct Claims {
    pub sub: String,

    #[serde(default)]
    pub preferred_username: Option<String>,

    #[serde(default)]
    pub email: Option<String>,

    #[serde(default)]
    pub groups: Option<Vec<String>>,
}

impl Claims {
    pub fn identity(&self) -> &str {
        self.preferred_username
            .as_deref()
            .or(self.email.as_deref())
            .unwrap_or(&self.sub)
    }
}

#[derive(Debug, Deserialize)]
struct UserInfo {
    #[serde(default)]
    groups: Vec<String>,
}

pub struct Groups;

impl Groups {
    pub fn matches(held: &[String], allowed: &[String]) -> bool {
        held.iter().any(|group| {
            let name = group
                .split_once('@')
                .map_or(group.as_str(), |(name, _)| name);

            allowed
                .iter()
                .any(|allowed| allowed == group || allowed == name)
        })
    }
}

#[derive(Clone)]
struct VerifyingKey {
    key: DecodingKey,
    alg: Option<Algorithm>,
}

#[derive(Clone)]
pub struct Authenticator {
    inner: Arc<AuthenticatorInner>,
}

struct AuthenticatorInner {
    client: reqwest::Client,
    issuer: String,
    audience: Option<String>,
    jwks_uri: String,
    userinfo_endpoint: String,
    groups: Vec<String>,
    keys: RwLock<BTreeMap<String, VerifyingKey>>,
}

impl Authenticator {
    const REFRESH_INTERVAL: Duration = Duration::from_secs(3600);

    pub fn new(config: &OAuthConfig) -> Self {
        let base = config.issuer.trim_end_matches('/');

        Self {
            inner: Arc::new(AuthenticatorInner {
                client: reqwest::Client::builder()
                    .timeout(Duration::from_secs(20))
                    .user_agent("ai-gateway")
                    .build()
                    .unwrap_or_default(),
                jwks_uri: config
                    .jwks_uri
                    .clone()
                    .unwrap_or_else(|| format!("{base}/public_key.jwk")),
                userinfo_endpoint: config
                    .userinfo_endpoint
                    .clone()
                    .unwrap_or_else(|| format!("{base}/userinfo")),
                issuer: config.issuer.clone(),
                audience: config.audience.clone(),
                groups: config.groups.clone(),
                keys: RwLock::new(BTreeMap::new()),
            }),
        }
    }

    pub async fn authorize(&self, token: &str) -> Result<Claims, AuthError> {
        let claims = self.verify(token).await?;
        let allowed = &self.inner.groups;

        if allowed.is_empty() {
            return Ok(claims);
        }

        let held = match &claims.groups {
            Some(groups) => groups.clone(),
            None => self.userinfo_groups(token).await?,
        };

        if !Groups::matches(&held, allowed) {
            return Err(AuthError::Forbidden(format!(
                "{} is not in any of {allowed:?}",
                claims.identity()
            )));
        }

        Ok(claims)
    }

    async fn userinfo_groups(&self, token: &str) -> Result<Vec<String>, AuthError> {
        let info: UserInfo = self
            .inner
            .client
            .get(&self.inner.userinfo_endpoint)
            .bearer_auth(token)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        Ok(info.groups)
    }

    pub async fn refresh(&self) -> Result<usize, AuthError> {
        let set: JwkSet = self
            .inner
            .client
            .get(&self.inner.jwks_uri)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        let mut keys = BTreeMap::new();

        for jwk in &set.keys {
            let Some(kid) = jwk.common.key_id.clone() else {
                continue;
            };

            if !Self::is_asymmetric(jwk) {
                tracing::warn!("ignoring non-asymmetric jwk {kid}");
                continue;
            }

            match DecodingKey::from_jwk(jwk) {
                Ok(key) => {
                    let alg = jwk
                        .common
                        .key_algorithm
                        .and_then(|a| a.to_string().parse::<Algorithm>().ok());

                    keys.insert(kid, VerifyingKey { key, alg });
                }
                Err(e) => tracing::warn!("ignoring unusable jwk {kid}: {e}"),
            }
        }

        if keys.is_empty() {
            return Err(AuthError::Jwks(format!(
                "no usable keys at {}",
                self.inner.jwks_uri
            )));
        }

        let count = keys.len();
        *self.inner.keys.write().await = keys;
        tracing::info!(keys = count, "refreshed jwks");

        Ok(count)
    }

    pub async fn run(self) {
        let mut ticker = tokio::time::interval(Self::REFRESH_INTERVAL);
        ticker.tick().await;

        loop {
            ticker.tick().await;

            if let Err(e) = self.refresh().await {
                tracing::warn!("jwks refresh failed, keeping previous keys: {e}");
            }
        }
    }

    pub async fn verify(&self, token: &str) -> Result<Claims, AuthError> {
        let header = decode_header(token)
            .map_err(|e| AuthError::Unauthorized(format!("unreadable token header: {e}")))?;

        let kid = header
            .kid
            .ok_or_else(|| AuthError::Unauthorized("token has no kid".to_owned()))?;

        let mut key = self.key(&kid).await;

        if key.is_none() {
            tracing::info!("unknown kid {kid}, refreshing jwks");
            self.refresh().await?;

            key = self.key(&kid).await;
        }

        let key = key.ok_or_else(|| AuthError::Unauthorized(format!("no key for kid {kid}")))?;

        let mut validation = Validation::new(key.alg.unwrap_or(header.alg));
        validation.set_issuer(&[&self.inner.issuer]);

        match &self.inner.audience {
            Some(audience) => validation.set_audience(&[audience]),
            None => validation.validate_aud = false,
        }

        decode::<Claims>(token, &key.key, &validation)
            .map(|data| data.claims)
            .map_err(|e| AuthError::Unauthorized(format!("token rejected: {e}")))
    }

    fn is_asymmetric(jwk: &Jwk) -> bool {
        matches!(
            jwk.algorithm,
            AlgorithmParameters::RSA(_)
                | AlgorithmParameters::EllipticCurve(_)
                | AlgorithmParameters::OctetKeyPair(_)
        )
    }

    async fn key(&self, kid: &str) -> Option<VerifyingKey> {
        self.inner.keys.read().await.get(kid).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_prefers_username_then_email_then_sub() {
        let mut claims = Claims {
            sub: "abc".to_owned(),
            preferred_username: Some("anurag".to_owned()),
            email: Some("hey@example.test".to_owned()),
            groups: None,
        };
        assert_eq!(claims.identity(), "anurag");

        claims.preferred_username = None;
        assert_eq!(claims.identity(), "hey@example.test");

        claims.email = None;
        assert_eq!(claims.identity(), "abc");
    }

    #[test]
    fn jwks_uri_defaults_to_the_issuer_path() {
        let auth = Authenticator::new(&OAuthConfig {
            issuer: "https://idm.example.test/oauth2/openid/app/".to_owned(),
            ..OAuthConfig::default()
        });

        assert_eq!(
            auth.inner.jwks_uri,
            "https://idm.example.test/oauth2/openid/app/public_key.jwk"
        );
        assert_eq!(
            auth.inner.userinfo_endpoint,
            "https://idm.example.test/oauth2/openid/app/userinfo"
        );
    }

    #[test]
    fn groups_match_by_short_name_or_spn() {
        let held = vec![
            "00000000-0000-0000-0000-000000000002".to_owned(),
            "ai_gateway_admins@idm.example.test".to_owned(),
        ];

        assert!(Groups::matches(&held, &["ai_gateway_admins".to_owned()]));
        assert!(Groups::matches(
            &held,
            &["ai_gateway_admins@idm.example.test".to_owned()]
        ));
        assert!(!Groups::matches(&held, &["platform_admins".to_owned()]));
        assert!(!Groups::matches(&held, &[]));
        assert!(!Groups::matches(&[], &["ai_gateway_admins".to_owned()]));
    }
}
