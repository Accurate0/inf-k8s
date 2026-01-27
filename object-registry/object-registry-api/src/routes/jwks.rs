use crate::{error::AppError, state::AppState};
use axum::{Json, extract::State};
use base64::{Engine, prelude::BASE64_URL_SAFE_NO_PAD};
use rsa::pkcs1::DecodeRsaPublicKey;
use rsa::traits::PublicKeyParts;
use rsa::{RsaPublicKey, pkcs8::DecodePublicKey};
use serde_json::{Value, json};

const PUBLIC_KEYS_PREFIX: &str = "public-keys/";

pub async fn get_jwks(State(state): State<AppState>) -> Result<Json<Value>, AppError> {
    let keys_list = state
        .object_manager
        .list_objects(PUBLIC_KEYS_PREFIX)
        .await?;

    let mut keys = Vec::new();

    for key in keys_list {
        if key.ends_with(".pem") {
            let parts: Vec<&str> = key.splitn(2, '/').collect();
            if parts.len() != 2 {
                continue;
            }
            let namespace = parts[0];
            let object = parts[1];

            // Use ObjectManager to fetch the key content
            // Assuming "public-keys" acts as the namespace here
            let stored = state
                .object_manager
                .get_object(namespace, object, None, false)
                .await?;

            let data = stored.data;
            let pem_str = String::from_utf8_lossy(&data);

            let public_key = if let Ok(pk) = RsaPublicKey::from_public_key_pem(&pem_str) {
                Some(pk)
            } else {
                RsaPublicKey::from_pkcs1_pem(&pem_str).ok()
            };

            if let Some(public_key) = public_key {
                let n = BASE64_URL_SAFE_NO_PAD.encode(public_key.n().to_bytes_be());
                let e = BASE64_URL_SAFE_NO_PAD.encode(public_key.e().to_bytes_be());

                let kid = key
                    .strip_prefix(PUBLIC_KEYS_PREFIX)
                    .unwrap_or(&key)
                    .strip_suffix(".pem")
                    .unwrap_or(&key);

                keys.push(json!({
                    "kty": "RSA",
                    "use": "sig",
                    "alg": "RS256",
                    "kid": kid,
                    "n": n,
                    "e": e,
                }));
            }
        }
    }

    Ok(Json(json!({ "keys": keys })))
}