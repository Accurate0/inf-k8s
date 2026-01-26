use crate::routes::objects::BUCKET_NAME;
use crate::{error::AppError, state::AppState};
use axum::{Json, extract::State};
use base64::{Engine, prelude::BASE64_URL_SAFE_NO_PAD};
use rsa::pkcs1::DecodeRsaPublicKey;
use rsa::traits::PublicKeyParts;
use rsa::{RsaPublicKey, pkcs8::DecodePublicKey};
use serde_json::{Value, json};

const PUBLIC_KEYS_PREFIX: &str = "public-keys/";

pub async fn get_jwks(State(state): State<AppState>) -> Result<Json<Value>, AppError> {
    let list_output = state
        .s3_client
        .list_objects_v2()
        .bucket(BUCKET_NAME)
        .prefix(PUBLIC_KEYS_PREFIX)
        .send()
        .await?;

    let mut keys = Vec::new();

    if let Some(objects) = list_output.contents {
        for object in objects {
            if let Some(key) = object.key {
                if key.ends_with(".pem") {
                    let get_output = state
                        .s3_client
                        .get_object()
                        .bucket(BUCKET_NAME)
                        .key(&key)
                        .send()
                        .await?;

                    let data = get_output.body.collect().await?.to_vec();
                    let pem_str = String::from_utf8_lossy(&data);

                    // Try parsing as PKCS#8 first
                    let public_key = if let Ok(pk) = RsaPublicKey::from_public_key_pem(&pem_str) {
                        Some(pk)
                    } else {
                        // Fallback to PKCS#1 if PKCS#8 fails
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
        }
    }

    Ok(Json(json!({ "keys": keys })))
}
