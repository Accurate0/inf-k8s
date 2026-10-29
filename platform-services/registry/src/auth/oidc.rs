use std::{collections::BTreeMap, sync::Arc, time::Duration};

use jsonwebtoken::{
    Algorithm, DecodingKey, Validation, decode, decode_header,
    jwk::{AlgorithmParameters, Jwk, JwkSet},
};
use serde::{Deserialize, de::DeserializeOwned};
use tokio::sync::RwLock;

#[derive(Debug, thiserror::Error)]
pub enum VerifyError {
    #[error("{0}")]
    Rejected(String),

    #[error("jwks error: {0}")]
    Jwks(String),

    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
}

#[derive(Clone)]
struct VerifyingKey {
    key: DecodingKey,
    alg: Option<Algorithm>,
}

#[derive(Clone)]
pub struct Verifier {
    inner: Arc<VerifierInner>,
}

struct VerifierInner {
    client: reqwest::Client,
    issuer: String,
    audience: Option<String>,
    jwks_uri: String,
    keys: RwLock<BTreeMap<String, VerifyingKey>>,
}

#[derive(Debug, Deserialize)]
struct UserInfo {
    #[serde(default)]
    groups: Vec<String>,
}

impl Verifier {
    const REFRESH_INTERVAL: Duration = Duration::from_secs(3600);

    pub fn new(issuer: &str, audience: Option<String>, jwks_uri: String) -> Self {
        Self {
            inner: Arc::new(VerifierInner {
                client: reqwest::Client::builder()
                    .timeout(Duration::from_secs(20))
                    .user_agent("registry")
                    .build()
                    .unwrap_or_default(),
                issuer: issuer.to_owned(),
                audience,
                jwks_uri,
                keys: RwLock::new(BTreeMap::new()),
            }),
        }
    }

    pub fn issuer(&self) -> &str {
        &self.inner.issuer
    }

    pub async fn verify<C: DeserializeOwned>(&self, token: &str) -> Result<C, VerifyError> {
        let header = decode_header(token)
            .map_err(|e| VerifyError::Rejected(format!("unreadable token header: {e}")))?;

        let kid = header
            .kid
            .ok_or_else(|| VerifyError::Rejected("token has no kid".to_owned()))?;

        let mut key = self.key(&kid).await;

        if key.is_none() {
            tracing::info!(
                issuer = self.inner.issuer,
                "unknown kid {kid}, refreshing jwks"
            );
            self.refresh().await?;

            key = self.key(&kid).await;
        }

        let key = key.ok_or_else(|| VerifyError::Rejected(format!("no key for kid {kid}")))?;

        let mut validation = Validation::new(key.alg.unwrap_or(header.alg));
        validation.set_issuer(&[&self.inner.issuer]);
        validation.leeway = 60;

        match &self.inner.audience {
            Some(audience) => validation.set_audience(&[audience]),
            None => validation.validate_aud = false,
        }

        decode::<C>(token, &key.key, &validation)
            .map(|data| data.claims)
            .map_err(|e| VerifyError::Rejected(format!("token rejected: {e}")))
    }

    pub async fn userinfo_groups(
        &self,
        endpoint: &str,
        token: &str,
    ) -> Result<Vec<String>, VerifyError> {
        let info: UserInfo = self
            .inner
            .client
            .get(endpoint)
            .bearer_auth(token)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        Ok(info.groups)
    }

    pub async fn refresh(&self) -> Result<usize, VerifyError> {
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
            return Err(VerifyError::Jwks(format!(
                "no usable keys at {}",
                self.inner.jwks_uri
            )));
        }

        let count = keys.len();
        *self.inner.keys.write().await = keys;
        tracing::info!(issuer = self.inner.issuer, keys = count, "refreshed jwks");

        Ok(count)
    }

    pub async fn run(self) {
        let mut ticker = tokio::time::interval(Self::REFRESH_INTERVAL);
        ticker.tick().await;

        loop {
            ticker.tick().await;

            if let Err(e) = self.refresh().await {
                tracing::warn!(
                    issuer = self.inner.issuer,
                    "jwks refresh failed, keeping previous keys: {e}"
                );
            }
        }
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

pub fn unverified_issuer(token: &str) -> Option<String> {
    use base64::Engine;

    #[derive(Deserialize)]
    struct Iss {
        iss: String,
    }

    let payload = token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .ok()?;
    serde_json::from_slice::<Iss>(&bytes).ok().map(|c| c.iss)
}

pub fn groups_match(held: &[String], allowed: &[String]) -> bool {
    held.iter().any(|group| {
        let name = group
            .split_once('@')
            .map_or(group.as_str(), |(name, _)| name);

        allowed
            .iter()
            .any(|allowed| allowed == group || allowed == name)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_issuer_without_verifying() {
        use base64::Engine;
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(br#"{"iss":"https://git.example.test/api/actions","sub":"x"}"#);
        let token = format!("e30.{payload}.sig");

        assert_eq!(
            unverified_issuer(&token).as_deref(),
            Some("https://git.example.test/api/actions")
        );
        assert_eq!(unverified_issuer("not-a-jwt"), None);
    }

    #[test]
    fn groups_match_by_short_name_or_spn() {
        let held = vec!["registry_admins@idm.example.test".to_owned()];

        assert!(groups_match(&held, &["registry_admins".to_owned()]));
        assert!(groups_match(
            &held,
            &["registry_admins@idm.example.test".to_owned()]
        ));
        assert!(!groups_match(&held, &["registry_users".to_owned()]));
        assert!(!groups_match(&held, &[]));
    }
}
