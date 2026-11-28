use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use jsonwebtoken::jwk::{AlgorithmParameters, JwkSet};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use serde::Deserialize;
use tokio::sync::{Mutex, RwLock};

use crate::access::{Grants, IdentityKind, kanidm::groups_match};
use crate::external::External;
use crate::passport::{self, Passport};

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

/// GraphQL-shaped so the router passes the reason on instead of rejecting a non-JSON body.
impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        let (status, code) = match &self {
            Self::Unauthorized(_) => (StatusCode::UNAUTHORIZED, "UNAUTHENTICATED"),
            Self::Forbidden(_) => (StatusCode::FORBIDDEN, "FORBIDDEN"),
            Self::Jwks(_) | Self::Http(_) => (StatusCode::SERVICE_UNAVAILABLE, "UNAVAILABLE"),
        };
        let body = serde_json::json!({
            "errors": [{ "message": self.to_string(), "extensions": { "code": code } }]
        });
        (status, axum::Json(body)).into_response()
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct AuthConfig {
    pub audience: String,
    pub jwks: String,
    #[serde(default)]
    pub grants: Grants,
}

impl AuthConfig {
    pub fn from_env() -> Result<Self, String> {
        let json = std::env::var("SUBGRAPH_AUTH").map_err(|_| "SUBGRAPH_AUTH is not set")?;
        Self::from_subgraph_auth(&json)
    }

    pub fn from_subgraph_auth(json: &str) -> Result<Self, String> {
        let config: Self =
            serde_json::from_str(json).map_err(|e| format!("SUBGRAPH_AUTH is not valid: {e}"))?;
        if config.audience.is_empty() || config.jwks.is_empty() {
            return Err("SUBGRAPH_AUTH needs an audience and the router's jwks".to_owned());
        }
        Ok(config)
    }
}

#[derive(Debug, Clone)]
pub enum Principal {
    User(Claims),
    External(External),
}

#[derive(Debug, Clone, Deserialize)]
pub struct Claims {
    pub sub: String,
    #[serde(default)]
    pub jti: Option<String>,
    #[serde(default)]
    pub preferred_username: Option<String>,
    #[serde(default)]
    pub groups: Option<Vec<String>>,
    /// Space-separated, as Kanidm issues it.
    #[serde(default)]
    pub scope: Option<String>,
}

impl Claims {
    pub fn identity(&self) -> &str {
        self.preferred_username.as_deref().unwrap_or(&self.sub)
    }

    pub fn scopes(&self) -> BTreeSet<&str> {
        self.scope
            .as_deref()
            .unwrap_or_default()
            .split_whitespace()
            .collect()
    }

    pub fn has_scope(&self, scope: &str) -> bool {
        self.scopes().contains(scope)
    }

    pub fn in_any(&self, allowed: &[&str]) -> bool {
        let allowed: Vec<String> = allowed.iter().map(|a| (*a).to_owned()).collect();
        groups_match(self.groups.as_deref().unwrap_or_default(), &allowed)
    }
}

impl Principal {
    pub fn scopes(&self) -> BTreeSet<String> {
        match self {
            Principal::User(claims) => claims.scopes().into_iter().map(str::to_owned).collect(),
            Principal::External(external) => external.scopes.clone(),
        }
    }
}

impl From<Passport> for Principal {
    fn from(passport: Passport) -> Self {
        let identity = passport.identity;
        match identity.kind {
            IdentityKind::User => Principal::User(Claims {
                sub: identity.subject,
                jti: None,
                preferred_username: identity.username,
                groups: Some(identity.groups),
                scope: Some(identity.scopes.into_iter().collect::<Vec<_>>().join(" ")),
            }),
            IdentityKind::External => Principal::External(External {
                issuer: identity.issuer,
                claims: identity.claims,
                scopes: identity.scopes,
            }),
        }
    }
}

#[derive(Clone)]
struct VerifyingKey {
    key: DecodingKey,
    alg: Option<Algorithm>,
}

#[derive(Clone)]
pub struct Authenticator {
    inner: Arc<Inner>,
}

struct Inner {
    client: reqwest::Client,
    config: AuthConfig,
    keys: RwLock<BTreeMap<String, VerifyingKey>>,
    last_refresh: Mutex<Option<Instant>>,
}

impl Authenticator {
    const REFRESH_INTERVAL: Duration = Duration::from_secs(3600);
    const UNKNOWN_KID_COOLDOWN: Duration = Duration::from_secs(30);

    pub fn new(config: AuthConfig) -> Self {
        Self {
            inner: Arc::new(Inner {
                client: reqwest::Client::builder()
                    .timeout(Duration::from_secs(20))
                    .user_agent("subgraph")
                    .build()
                    .unwrap_or_default(),
                config,
                keys: RwLock::new(BTreeMap::new()),
                last_refresh: Mutex::new(None),
            }),
        }
    }

    pub fn config(&self) -> &AuthConfig {
        &self.inner.config
    }

    pub async fn authenticate(&self, passport: &str) -> Result<Principal, AuthError> {
        self.verify(passport).await.map(Principal::from)
    }

    pub async fn authenticate_headers(&self, headers: &HeaderMap) -> Result<Principal, AuthError> {
        let passport = headers
            .get(passport::HEADER)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| AuthError::Unauthorized("missing graph passport".to_owned()))?;
        self.authenticate(passport).await
    }

    pub async fn refresh(&self) -> Result<usize, AuthError> {
        let mut last = self.inner.last_refresh.lock().await;
        *last = Some(Instant::now());
        self.fetch_keys().await
    }

    // an in-flight refresh may bring the kid; otherwise refresh at most once per cooldown
    async fn refresh_for(&self, kid: &str) -> Result<(), AuthError> {
        let mut last = self.inner.last_refresh.lock().await;
        let known = self.inner.keys.read().await.contains_key(kid);
        if known || last.is_some_and(|at| at.elapsed() < Self::UNKNOWN_KID_COOLDOWN) {
            return Ok(());
        }

        tracing::info!("unknown passport kid {kid}, refreshing jwks");
        *last = Some(Instant::now());
        self.fetch_keys().await.map(|_| ())
    }

    async fn fetch_keys(&self) -> Result<usize, AuthError> {
        let jwks = &self.inner.config.jwks;

        let set: JwkSet = self
            .inner
            .client
            .get(jwks)
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
            let asymmetric = matches!(
                jwk.algorithm,
                AlgorithmParameters::RSA(_)
                    | AlgorithmParameters::EllipticCurve(_)
                    | AlgorithmParameters::OctetKeyPair(_)
            );
            if !asymmetric {
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
            return Err(AuthError::Jwks(format!("no usable keys at {jwks}")));
        }

        let count = keys.len();
        *self.inner.keys.write().await = keys;
        tracing::info!(jwks, keys = count, "refreshed passport jwks");
        Ok(count)
    }

    /// Refreshes keys hourly; unknown key ids also trigger a refresh on demand.
    pub async fn run(self) {
        if let Err(e) = self.refresh().await {
            tracing::warn!("initial passport jwks fetch failed, retrying on demand: {e}");
        }
        let mut ticker = tokio::time::interval(Self::REFRESH_INTERVAL);
        ticker.tick().await;
        loop {
            ticker.tick().await;
            if let Err(e) = self.refresh().await {
                tracing::warn!("passport jwks refresh failed, keeping previous keys: {e}");
            }
        }
    }

    async fn verify(&self, token: &str) -> Result<Passport, AuthError> {
        let header = decode_header(token)
            .map_err(|e| AuthError::Unauthorized(format!("unreadable passport header: {e}")))?;
        let kid = header
            .kid
            .ok_or_else(|| AuthError::Unauthorized("passport has no kid".to_owned()))?;

        let mut key = self.inner.keys.read().await.get(&kid).cloned();
        if key.is_none() {
            self.refresh_for(&kid).await?;
            key = self.inner.keys.read().await.get(&kid).cloned();
        }
        let key = key.ok_or_else(|| AuthError::Unauthorized(format!("no key for kid {kid}")))?;

        let mut validation = Validation::new(key.alg.unwrap_or(header.alg));
        validation.set_issuer(&[passport::ISSUER]);
        validation.set_audience(&[&self.inner.config.audience]);
        validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
        validation.leeway = 5;

        decode::<Passport>(token, &key.key, &validation)
            .map(|data| data.claims)
            .map_err(|e| AuthError::Unauthorized(format!("passport rejected: {e}")))
    }
}

pub fn bearer(value: &str) -> Option<&str> {
    let token = value.trim().strip_prefix("Bearer ")?.trim();
    (!token.is_empty()).then_some(token)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::access::Identity;
    use crate::passport::Minter;

    const SEED: &str = "0123456789abcdefghijklmnopqrstuvwxyz";

    fn identity(kind: IdentityKind) -> Identity {
        Identity {
            issuer: match kind {
                IdentityKind::User => "kanidm".to_owned(),
                IdentityKind::External => "forgejo".to_owned(),
            },
            kind,
            token_issuer: "https://idm.example.test/oauth2/openid/graph".to_owned(),
            subject: "uuid".to_owned(),
            username: Some("anurag".to_owned()),
            client: Some("graph".to_owned()),
            scopes: ["registry:package:read".to_owned()].into(),
            groups: vec!["registry_users@idm.example.test".to_owned()],
            claims: serde_json::json!({ "repository": "anurag/k8s" }),
            expires_at: None,
        }
    }

    async fn authenticator(minter: &Minter) -> Authenticator {
        let auth = Authenticator::new(AuthConfig {
            audience: "registry".to_owned(),
            jwks: "http://router.invalid/.well-known/passport.jwks".to_owned(),
            grants: Grants::default(),
        });
        let set: JwkSet = serde_json::from_str(minter.jwks()).unwrap();
        let jwk = &set.keys[0];
        auth.inner.keys.write().await.insert(
            jwk.common.key_id.clone().unwrap(),
            VerifyingKey {
                key: DecodingKey::from_jwk(jwk).unwrap(),
                alg: Some(Algorithm::ES256),
            },
        );
        *auth.inner.last_refresh.lock().await = Some(Instant::now());
        auth
    }

    #[tokio::test]
    async fn user_passports_become_claims() {
        let minter = Minter::from_seed(SEED, Duration::from_secs(60)).unwrap();
        let auth = authenticator(&minter).await;

        let passport = minter
            .mint(&identity(IdentityKind::User), "registry")
            .unwrap();
        let Principal::User(claims) = auth.authenticate(&passport).await.unwrap() else {
            panic!("expected a user");
        };
        assert_eq!(claims.identity(), "anurag");
        assert!(claims.has_scope("registry:package:read"));
        assert!(claims.in_any(&["registry_users"]));
    }

    #[tokio::test]
    async fn external_passports_keep_their_claims_and_scopes() {
        let minter = Minter::from_seed(SEED, Duration::from_secs(60)).unwrap();
        let auth = authenticator(&minter).await;

        let passport = minter
            .mint(&identity(IdentityKind::External), "registry")
            .unwrap();
        let Principal::External(external) = auth.authenticate(&passport).await.unwrap() else {
            panic!("expected an external identity");
        };
        assert_eq!(external.issuer, "forgejo");
        assert_eq!(external.claims["repository"], "anurag/k8s");
        assert!(external.scopes.contains("registry:package:read"));
    }

    #[tokio::test]
    async fn passports_for_another_subgraph_are_rejected() {
        let minter = Minter::from_seed(SEED, Duration::from_secs(60)).unwrap();
        let auth = authenticator(&minter).await;
        let passport = minter.mint(&identity(IdentityKind::User), "waf").unwrap();
        assert!(matches!(
            auth.authenticate(&passport).await,
            Err(AuthError::Unauthorized(_))
        ));
    }

    #[tokio::test]
    async fn passports_signed_with_another_key_are_rejected() {
        let minter = Minter::from_seed(SEED, Duration::from_secs(60)).unwrap();
        let auth = authenticator(&minter).await;
        let forger = Minter::from_seed(
            "a different seed of at least 32 chars",
            Duration::from_secs(60),
        )
        .unwrap();
        let passport = forger
            .mint(&identity(IdentityKind::User), "registry")
            .unwrap();
        assert!(matches!(
            auth.authenticate(&passport).await,
            Err(AuthError::Unauthorized(_))
        ));
    }

    #[tokio::test]
    async fn expired_passports_are_rejected() {
        let minter = Minter::from_seed(SEED, Duration::from_secs(60)).unwrap();
        let auth = authenticator(&minter).await;
        let an_hour_ago = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
            - 3600;
        let passport = minter
            .mint_at(&identity(IdentityKind::User), "registry", an_hour_ago)
            .unwrap();
        assert!(matches!(
            auth.authenticate(&passport).await,
            Err(AuthError::Unauthorized(_))
        ));
    }

    #[tokio::test]
    async fn requests_without_a_passport_are_unauthorized() {
        let minter = Minter::from_seed(SEED, Duration::from_secs(60)).unwrap();
        let auth = authenticator(&minter).await;
        let mut headers = HeaderMap::new();
        headers.insert("authorization", "Bearer kanidm-token".parse().unwrap());
        assert!(matches!(
            auth.authenticate_headers(&headers).await,
            Err(AuthError::Unauthorized(_))
        ));
    }

    #[test]
    fn seeds_derive_one_stable_key() {
        let a = Minter::from_seed(SEED, Duration::from_secs(60)).unwrap();
        let b = Minter::from_seed(SEED, Duration::from_secs(60)).unwrap();
        assert_eq!(a.kid(), b.kid());
        assert_eq!(a.jwks(), b.jwks());
        assert!(Minter::from_seed("short", Duration::from_secs(60)).is_err());
    }

    #[tokio::test]
    async fn unknown_kids_refresh_at_most_once_per_cooldown() {
        let auth = Authenticator::new(AuthConfig {
            audience: "registry".to_owned(),
            jwks: "http://router.invalid/jwks".to_owned(),
            grants: Grants::default(),
        });
        // nothing fetched yet, so an unknown kid fetches (and fails, the jwks is unreachable)
        assert!(auth.refresh_for("k").await.is_err());
        // within the cooldown it doesn't fetch again
        assert!(auth.refresh_for("k").await.is_ok());
    }

    #[test]
    fn subgraph_auth_needs_an_audience_and_jwks() {
        let config = AuthConfig::from_subgraph_auth(
            r#"{"audience":"registry","jwks":"http://router/.well-known/passport.jwks",
                "grants":{"registry:package:publish":{"forgejo":[{"repository":"anurag/*"}]}}}"#,
        )
        .unwrap();
        assert_eq!(config.audience, "registry");
        let granted = crate::access::external::granted(
            &config.grants,
            "forgejo",
            &serde_json::json!({ "repository": "anurag/k8s" }),
        );
        assert_eq!(
            granted,
            BTreeSet::from(["registry:package:publish".to_owned()])
        );

        assert!(AuthConfig::from_subgraph_auth(r#"{"audience":"","jwks":"x"}"#).is_err());
        assert!(AuthConfig::from_subgraph_auth(r#"{"providers":[]}"#).is_err());
    }

    #[tokio::test]
    async fn errors_are_graphql_json() {
        let response = AuthError::Unauthorized("missing graph passport".to_owned()).into_response();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            response.headers()[axum::http::header::CONTENT_TYPE],
            "application/json"
        );
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            body["errors"][0]["message"],
            "unauthorized: missing graph passport"
        );
        assert_eq!(body["errors"][0]["extensions"]["code"], "UNAUTHENTICATED");
    }

    #[test]
    fn user_scopes_come_from_the_space_separated_claim() {
        let claims: Claims = serde_json::from_value(serde_json::json!({
            "sub": "u", "scope": "openid email registry:package:read"
        }))
        .unwrap();
        assert!(claims.has_scope("registry:package:read"));
        assert!(!claims.has_scope("registry:token:create"));
        assert!(Principal::User(claims).scopes().contains("openid"));
    }
}
