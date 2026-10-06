use crate::config::Config;
use futures::future::BoxFuture;
use jsonwebtoken::jwk::{AlgorithmParameters, Jwk, JwkSet};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use moka::future::Cache;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::sync::RwLock;
use tonic::Status;
use tower::{Layer, Service};

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("unauthorized: {0}")]
    Unauthorized(String),

    #[error("jwks error: {0}")]
    Jwks(String),

    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Actor(pub String);

#[derive(Debug, Clone, Deserialize)]
pub struct Claims {
    pub sub: String,

    #[serde(default)]
    pub jti: Option<String>,

    #[serde(default)]
    pub preferred_username: Option<String>,

    #[serde(default)]
    pub email: Option<String>,
}

impl Claims {
    pub fn identity(&self) -> Option<String> {
        self.preferred_username
            .as_deref()
            .or(self.email.as_deref())
            .map(str::to_owned)
    }

    fn cache_key(&self) -> String {
        self.jti.clone().unwrap_or_else(|| self.sub.clone())
    }
}

#[derive(Debug, Deserialize)]
struct UserInfo {
    #[serde(default)]
    preferred_username: Option<String>,

    #[serde(default)]
    email: Option<String>,
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
    keys: RwLock<BTreeMap<String, VerifyingKey>>,
    actors: Cache<String, String>,
}

impl Authenticator {
    const ACTOR_TTL: Duration = Duration::from_secs(300);
    const REFRESH_INTERVAL: Duration = Duration::from_secs(3600);

    pub fn new(
        issuer: impl Into<String>,
        audience: Option<String>,
        jwks_uri: Option<String>,
        userinfo_endpoint: Option<String>,
    ) -> Self {
        let issuer = issuer.into();
        let base = issuer.trim_end_matches('/').to_owned();

        Self {
            inner: Arc::new(AuthenticatorInner {
                client: reqwest::Client::builder()
                    .timeout(Duration::from_secs(20))
                    .user_agent("feature-flags")
                    .build()
                    .unwrap_or_default(),
                jwks_uri: jwks_uri.unwrap_or_else(|| format!("{base}/public_key.jwk")),
                userinfo_endpoint: userinfo_endpoint.unwrap_or_else(|| format!("{base}/userinfo")),
                issuer,
                audience,
                keys: RwLock::new(BTreeMap::new()),
                actors: Cache::builder()
                    .max_capacity(1024)
                    .time_to_live(Self::ACTOR_TTL)
                    .build(),
            }),
        }
    }

    pub fn from_config(config: &Config) -> Option<Self> {
        let issuer = config.oidc_issuer.clone().filter(|s| !s.is_empty())?;

        Some(Self::new(
            issuer,
            config.oidc_audience.clone().filter(|s| !s.is_empty()),
            config.oidc_jwks_uri.clone().filter(|s| !s.is_empty()),
            config
                .oidc_userinfo_endpoint
                .clone()
                .filter(|s| !s.is_empty()),
        ))
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

            if let Err(e) = self.refresh().await {
                tracing::warn!("jwks refresh for unknown kid failed: {e}");
            }

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

    pub async fn authenticate(&self, token: &str) -> Result<Actor, AuthError> {
        let claims = self.verify(token).await?;

        if let Some(identity) = claims.identity() {
            return Ok(Actor(identity));
        }

        let cache_key = claims.cache_key();

        if let Some(identity) = self.inner.actors.get(&cache_key).await {
            return Ok(Actor(identity));
        }

        let Some(identity) = self.userinfo(token).await else {
            return Ok(Actor(claims.sub));
        };

        self.inner.actors.insert(cache_key, identity.clone()).await;

        Ok(Actor(identity))
    }

    async fn userinfo(&self, token: &str) -> Option<String> {
        let response = self
            .inner
            .client
            .get(&self.inner.userinfo_endpoint)
            .bearer_auth(token)
            .send()
            .await
            .and_then(reqwest::Response::error_for_status);

        let info: UserInfo = match response {
            Ok(response) => response.json().await.ok()?,
            Err(e) => {
                tracing::warn!("userinfo lookup failed: {e}");
                return None;
            }
        };

        info.preferred_username.or(info.email)
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

#[derive(Clone)]
pub struct AuthLayer {
    authenticator: Option<Authenticator>,
}

impl AuthLayer {
    const OPEN_PREFIXES: [&'static str; 2] =
        ["/featureflag.v1.Evaluation/", "/grpc.health.v1.Health/"];

    pub fn new(authenticator: Option<Authenticator>) -> Self {
        Self { authenticator }
    }

    pub fn is_open(path: &str) -> bool {
        Self::OPEN_PREFIXES
            .iter()
            .any(|prefix| path.starts_with(prefix))
    }

    fn bearer(headers: &http::HeaderMap) -> Option<&str> {
        let value = headers.get(http::header::AUTHORIZATION)?.to_str().ok()?;
        let (scheme, token) = value.split_once(' ')?;

        (scheme.eq_ignore_ascii_case("bearer") && !token.trim().is_empty()).then(|| token.trim())
    }
}

impl<S> Layer<S> for AuthLayer {
    type Service = AuthService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        AuthService {
            inner,
            authenticator: self.authenticator.clone(),
        }
    }
}

#[derive(Clone)]
pub struct AuthService<S> {
    inner: S,
    authenticator: Option<Authenticator>,
}

impl<S, B, R> Service<http::Request<B>> for AuthService<S>
where
    S: Service<http::Request<B>, Response = http::Response<R>> + Clone + Send + 'static,
    S::Future: Send + 'static,
    B: Send + 'static,
    R: Default + Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = BoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut request: http::Request<B>) -> Self::Future {
        let clone = self.inner.clone();
        let mut inner = std::mem::replace(&mut self.inner, clone);
        let authenticator = self.authenticator.clone();

        Box::pin(async move {
            let Some(authenticator) = authenticator else {
                return inner.call(request).await;
            };

            if AuthLayer::is_open(request.uri().path()) {
                return inner.call(request).await;
            }

            let Some(token) = AuthLayer::bearer(request.headers()).map(str::to_owned) else {
                return Ok(Status::unauthenticated("missing bearer token").into_http());
            };

            match authenticator.authenticate(&token).await {
                Ok(actor) => {
                    request.extensions_mut().insert(actor);
                    inner.call(request).await
                }
                Err(AuthError::Unauthorized(reason)) => {
                    Ok(Status::unauthenticated(reason).into_http())
                }
                Err(e) => {
                    tracing::error!("authentication failed: {e}");
                    Ok(Status::unavailable("authentication unavailable").into_http())
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evaluation_and_health_paths_are_open() {
        assert!(AuthLayer::is_open(
            "/featureflag.v1.Evaluation/ResolveBoolean"
        ));
        assert!(AuthLayer::is_open(
            "/featureflag.v1.Evaluation/StreamSnapshot"
        ));
        assert!(AuthLayer::is_open("/grpc.health.v1.Health/Check"));
    }

    #[test]
    fn every_other_path_requires_a_token() {
        assert!(!AuthLayer::is_open("/featureflag.v1.Admin/ListFlags"));
        assert!(!AuthLayer::is_open("/featureflag.v1.Admin/ApplyConfig"));
        assert!(!AuthLayer::is_open(
            "/grpc.reflection.v1.ServerReflection/ServerReflectionInfo"
        ));
        assert!(!AuthLayer::is_open("/featureflag.v1.EvaluationX/Anything"));
        assert!(!AuthLayer::is_open("/"));
    }

    #[test]
    fn bearer_is_extracted_case_insensitively() {
        let mut headers = http::HeaderMap::new();
        assert_eq!(AuthLayer::bearer(&headers), None);

        headers.insert(http::header::AUTHORIZATION, "bearer abc".parse().unwrap());
        assert_eq!(AuthLayer::bearer(&headers), Some("abc"));

        headers.insert(http::header::AUTHORIZATION, "Basic abc".parse().unwrap());
        assert_eq!(AuthLayer::bearer(&headers), None);

        headers.insert(http::header::AUTHORIZATION, "Bearer ".parse().unwrap());
        assert_eq!(AuthLayer::bearer(&headers), None);
    }

    #[test]
    fn identity_prefers_username_then_email() {
        let mut claims = Claims {
            sub: "abc".to_owned(),
            jti: None,
            preferred_username: Some("anurag".to_owned()),
            email: Some("hey@example.test".to_owned()),
        };
        assert_eq!(claims.identity().as_deref(), Some("anurag"));

        claims.preferred_username = None;
        assert_eq!(claims.identity().as_deref(), Some("hey@example.test"));

        claims.email = None;
        assert_eq!(claims.identity(), None);
        assert_eq!(claims.cache_key(), "abc");
    }

    #[test]
    fn endpoints_default_to_the_issuer_paths() {
        let auth = Authenticator::new(
            "https://idm.example.test/oauth2/openid/app/",
            None,
            None,
            None,
        );

        assert_eq!(
            auth.inner.jwks_uri,
            "https://idm.example.test/oauth2/openid/app/public_key.jwk"
        );
        assert_eq!(
            auth.inner.userinfo_endpoint,
            "https://idm.example.test/oauth2/openid/app/userinfo"
        );
    }
}
