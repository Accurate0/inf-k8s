use crate::{error::AppError, state::AppState};
use axum::{Json, extract::State};
use base64::{Engine, prelude::BASE64_URL_SAFE_NO_PAD};
use lambda_http::tracing;
use rsa::pkcs1::DecodeRsaPublicKey;
use rsa::traits::PublicKeyParts;
use rsa::{RsaPublicKey, pkcs8::DecodePublicKey};
use serde_json::{Value, json};

const PUBLIC_KEYS_BUCKET: &str = "object-registry-public-keys-inf-k8s";

pub async fn get_jwks(State(state): State<AppState>) -> Result<Json<Value>, AppError> {
    let object_contents = state
        .s3_client
        .list_objects_v2()
        .bucket(PUBLIC_KEYS_BUCKET)
        .send()
        .await?;

    let mut keys = Vec::new();

    for object in object_contents.contents() {
        let Some(object_key) = object.key() else {
            tracing::warn!("object without key? {object:?}");
            continue;
        };

        if object_key.ends_with(".pem") {
            let stored_object = state
                .s3_client
                .get_object()
                .bucket(PUBLIC_KEYS_BUCKET)
                .key(object_key)
                .send()
                .await?;

            let data = stored_object.body.collect().await?.into_bytes();
            let pem_str = String::from_utf8_lossy(&data);

            let public_key = if let Ok(pk) = RsaPublicKey::from_public_key_pem(&pem_str) {
                Some(pk)
            } else {
                RsaPublicKey::from_pkcs1_pem(&pem_str).ok()
            };

            if let Some(public_key) = public_key {
                let n = BASE64_URL_SAFE_NO_PAD.encode(public_key.n().to_bytes_be());
                let e = BASE64_URL_SAFE_NO_PAD.encode(public_key.e().to_bytes_be());

                let kid = object_key.strip_suffix(".pem").unwrap_or(&object_key);

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
