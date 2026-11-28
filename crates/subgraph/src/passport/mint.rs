use std::time::{Duration, SystemTime, UNIX_EPOCH};

use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use p256::pkcs8::EncodePrivateKey;
use sha2::{Digest, Sha256};

use super::{ISSUER, Passport};
use crate::access::Identity;

// the key derives from a generated seed, so nobody handles it; a new seed rotates it
pub struct Minter {
    key: EncodingKey,
    kid: String,
    jwks: String,
    ttl: Duration,
}

impl Minter {
    pub fn from_seed(seed: &str, ttl: Duration) -> Result<Self, String> {
        if seed.len() < 32 {
            return Err("the passport seed needs at least 32 characters".to_owned());
        }

        let secret = p256::SecretKey::from_slice(Sha256::digest(seed.as_bytes()).as_slice())
            .map_err(|e| format!("unusable passport seed: {e}"))?;
        let der = secret
            .to_pkcs8_der()
            .map_err(|e| format!("passport key: {e}"))?;

        let public = secret.public_key();
        let kid: String = Sha256::digest(public.to_sec1_bytes())
            .iter()
            .take(8)
            .map(|byte| format!("{byte:02x}"))
            .collect();

        let mut jwk: serde_json::Value = serde_json::from_str(&public.to_jwk_string())
            .map_err(|e| format!("passport jwk: {e}"))?;
        jwk["kid"] = kid.clone().into();
        jwk["alg"] = "ES256".into();
        jwk["use"] = "sig".into();

        Ok(Self {
            key: EncodingKey::from_ec_der(der.as_bytes()),
            jwks: serde_json::json!({ "keys": [jwk] }).to_string(),
            kid,
            ttl,
        })
    }

    pub fn jwks(&self) -> &str {
        &self.jwks
    }

    pub fn kid(&self) -> &str {
        &self.kid
    }

    pub fn mint(&self, identity: &Identity, audience: &str) -> Result<String, String> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_secs() as i64;
        self.mint_at(identity, audience, now)
    }

    pub(crate) fn mint_at(
        &self,
        identity: &Identity,
        audience: &str,
        now: i64,
    ) -> Result<String, String> {
        let passport = Passport {
            iss: ISSUER.to_owned(),
            aud: audience.to_owned(),
            sub: identity.subject.clone(),
            iat: now,
            exp: now + self.ttl.as_secs() as i64,
            identity: identity.clone(),
        };

        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some(self.kid.clone());
        encode(&header, &passport, &self.key).map_err(|e| format!("passport signing: {e}"))
    }
}
