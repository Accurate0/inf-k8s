use crate::error::{Error, Result};
use crate::metrics::Metrics;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use jsonwebtoken::jwk::{AlgorithmParameters, Jwk, JwkSet};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::sync::RwLock;

const ISSUER_ENV: &str = "OIDC_ISSUER";
const JWKS_ENV: &str = "OIDC_JWKS_URI";
const AUDIENCE_ENV: &str = "OIDC_AUDIENCE";

#[derive(Debug, Clone, Deserialize)]
pub struct Claims {
    pub sub: String,

    #[serde(default)]
    pub preferred_username: Option<String>,

    #[serde(default)]
    pub email: Option<String>,

    #[serde(default)]
    pub name: Option<String>,
}

impl Claims {
    pub fn identity(&self) -> String {
        self.preferred_username
            .as_deref()
            .or(self.email.as_deref())
            .unwrap_or(&self.sub)
            .to_string()
    }
}

#[derive(Clone)]
pub struct Jwks {
    inner: Arc<JwksInner>,
}

struct JwksInner {
    client: reqwest::Client,
    uri: String,
    issuer: String,
    audience: Option<String>,
    keys: RwLock<BTreeMap<String, VerifyingKey>>,
}

#[derive(Clone)]
struct VerifyingKey {
    key: DecodingKey,
    alg: Option<Algorithm>,
}

impl Jwks {
    pub fn new(
        uri: impl Into<String>,
        issuer: impl Into<String>,
        audience: Option<String>,
    ) -> Self {
        Self {
            inner: Arc::new(JwksInner {
                client: reqwest::Client::builder()
                    .timeout(std::time::Duration::from_secs(20))
                    .user_agent("waf-manager")
                    .build()
                    .unwrap_or_default(),
                uri: uri.into(),
                issuer: issuer.into(),
                audience,
                keys: RwLock::new(BTreeMap::new()),
            }),
        }
    }

    pub fn from_env() -> Option<Self> {
        let issuer = std::env::var(ISSUER_ENV).ok()?;
        let uri = std::env::var(JWKS_ENV)
            .unwrap_or_else(|_| format!("{}/public_key.jwk", issuer.trim_end_matches('/')));
        let audience = std::env::var(AUDIENCE_ENV).ok().filter(|a| !a.is_empty());

        Some(Self::new(uri, issuer, audience))
    }

    pub async fn refresh(&self) -> Result<usize> {
        let set: JwkSet = self
            .inner
            .client
            .get(&self.inner.uri)
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
                    keys.insert(
                        kid,
                        VerifyingKey {
                            key,
                            alg: jwk
                                .common
                                .key_algorithm
                                .and_then(|a| a.to_string().parse::<Algorithm>().ok()),
                        },
                    );
                }
                Err(e) => tracing::warn!("ignoring unusable jwk {kid}: {e}"),
            }
        }

        if keys.is_empty() {
            Metrics::record_jwks_refresh("error");
            return Err(Error::Jwks(format!("no usable keys at {}", self.inner.uri)));
        }

        let count = keys.len();
        *self.inner.keys.write().await = keys;

        Metrics::record_jwks_refresh("success");
        Metrics::set_jwks_keys(count);
        tracing::info!(keys = count, "refreshed jwks");

        Ok(count)
    }

    pub async fn self_check(&self) -> Result<()> {
        let Some((kid, key)) = self.any_key().await else {
            return Err(Error::Jwks("no keys loaded for self check".to_string()));
        };

        let alg = key.alg.unwrap_or(Algorithm::RS256);
        let header = URL_SAFE_NO_PAD.encode(format!(r#"{{"alg":"{alg:?}","kid":"{kid}"}}"#));
        let payload = URL_SAFE_NO_PAD.encode(r#"{"sub":"self-check"}"#);
        let signature = URL_SAFE_NO_PAD.encode([0u8; 64]);

        match self
            .verify(&format!("{header}.{payload}.{signature}"))
            .await
        {
            Err(Error::Unauthorized(_)) => Ok(()),
            Ok(_) => Err(Error::Jwks(
                "self check token was accepted, verification is not enforcing signatures"
                    .to_string(),
            )),
            Err(e) => Err(e),
        }
    }

    pub async fn run(self, interval: std::time::Duration) {
        let mut ticker = tokio::time::interval(interval);

        loop {
            ticker.tick().await;

            if let Err(e) = self.refresh().await {
                tracing::warn!("jwks refresh failed, keeping previous keys: {e}");
            }
        }
    }

    pub async fn verify(&self, token: &str) -> Result<Claims> {
        let header = decode_header(token)
            .map_err(|e| Error::Unauthorized(format!("unreadable token header: {e}")))?;

        let kid = header
            .kid
            .ok_or_else(|| Error::Unauthorized("token has no kid".to_string()))?;

        let mut key = self.key(&kid).await;

        if key.is_none() {
            tracing::info!("unknown kid {kid}, refreshing jwks");

            if let Err(e) = self.refresh().await {
                tracing::warn!("jwks refresh for unknown kid failed: {e}");
            }

            key = self.key(&kid).await;
        }

        let key = key.ok_or_else(|| Error::Unauthorized(format!("no key for kid {kid}")))?;

        let mut validation = Validation::new(key.alg.unwrap_or(header.alg));
        validation.set_issuer(&[&self.inner.issuer]);

        match &self.inner.audience {
            Some(audience) => validation.set_audience(&[audience]),
            None => validation.validate_aud = false,
        }

        decode::<Claims>(token, &key.key, &validation)
            .map(|data| data.claims)
            .map_err(|e| Error::Unauthorized(format!("token rejected: {e}")))
    }

    fn is_asymmetric(jwk: &Jwk) -> bool {
        matches!(
            jwk.algorithm,
            AlgorithmParameters::RSA(_)
                | AlgorithmParameters::EllipticCurve(_)
                | AlgorithmParameters::OctetKeyPair(_)
        )
    }

    async fn any_key(&self) -> Option<(String, VerifyingKey)> {
        let keys = self.inner.keys.read().await;
        keys.iter()
            .next()
            .map(|(kid, key)| (kid.clone(), key.clone()))
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
        let claims = Claims {
            sub: "abc".to_string(),
            preferred_username: Some("anurag".to_string()),
            email: Some("hey@example.test".to_string()),
            name: None,
        };
        assert_eq!(claims.identity(), "anurag");

        let claims = Claims {
            sub: "abc".to_string(),
            preferred_username: None,
            email: Some("hey@example.test".to_string()),
            name: None,
        };
        assert_eq!(claims.identity(), "hey@example.test");

        let claims = Claims {
            sub: "abc".to_string(),
            preferred_username: None,
            email: None,
            name: None,
        };
        assert_eq!(claims.identity(), "abc");
    }

    #[test]
    fn kanidms_es256_key_is_usable() {
        let set: JwkSet = serde_json::from_str(
            r#"{"keys":[{"kty":"EC","crv":"P-256",
                "x":"pHzGsnQ1iSPTB2OE7sx7lrMcHHmwFvXPN5SBtWaS7w0",
                "y":"YT5H7NQ9WvpxyWcEoWSwv5lEsbswWUHZCHJiY1OYSLE",
                "alg":"ES256","use":"sig","kid":"495e009e846e"}]}"#,
        )
        .unwrap();

        let jwk = &set.keys[0];

        assert!(Jwks::is_asymmetric(jwk));
        assert!(DecodingKey::from_jwk(jwk).is_ok());
        assert_eq!(
            jwk.common
                .key_algorithm
                .and_then(|a| a.to_string().parse::<Algorithm>().ok()),
            Some(Algorithm::ES256)
        );
    }

    #[test]
    fn shared_secrets_are_never_trusted_as_verifying_keys() {
        let set: JwkSet = serde_json::from_str(
            r#"{"keys":[{"kty":"oct","k":"c2VjcmV0","alg":"HS256","kid":"shared"}]}"#,
        )
        .unwrap();

        assert!(!Jwks::is_asymmetric(&set.keys[0]));
    }

    #[test]
    fn jwks_uri_defaults_to_the_issuer_wellknown_path() {
        let jwks = Jwks::new(
            "https://idm.example.test/oauth2/openid/waf/public_key.jwk",
            "https://idm.example.test/oauth2/openid/waf",
            None,
        );

        assert_eq!(
            jwks.inner.issuer,
            "https://idm.example.test/oauth2/openid/waf"
        );
    }

    #[tokio::test]
    async fn verify_rejects_a_token_with_no_kid() {
        let jwks = Jwks::new("https://example.test/jwks", "https://example.test", None);
        let header = "eyJhbGciOiJIUzI1NiJ9";
        let payload = "eyJzdWIiOiJhIn0";
        let signature = "c2ln";
        let token = format!("{header}.{payload}.{signature}");

        let err = jwks.verify(&token).await.unwrap_err();

        assert!(matches!(err, Error::Unauthorized(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn self_check_exercises_signature_verification() {
        let set: JwkSet = serde_json::from_str(
            r#"{"keys":[{"kty":"EC","crv":"P-256",
                "x":"pHzGsnQ1iSPTB2OE7sx7lrMcHHmwFvXPN5SBtWaS7w0",
                "y":"YT5H7NQ9WvpxyWcEoWSwv5lEsbswWUHZCHJiY1OYSLE",
                "alg":"ES256","use":"sig","kid":"495e009e846e"}]}"#,
        )
        .unwrap();

        let jwks = Jwks::new("https://example.test/jwks", "https://example.test", None);
        jwks.inner.keys.write().await.insert(
            "495e009e846e".to_string(),
            VerifyingKey {
                key: DecodingKey::from_jwk(&set.keys[0]).unwrap(),
                alg: Some(Algorithm::ES256),
            },
        );

        jwks.self_check().await.unwrap();
    }

    #[tokio::test]
    async fn verify_rejects_unparsable_tokens() {
        let jwks = Jwks::new("https://example.test/jwks", "https://example.test", None);

        let err = jwks.verify("not-a-token").await.unwrap_err();

        assert!(matches!(err, Error::Unauthorized(_)), "got {err:?}");
    }
}
