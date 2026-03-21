use crate::{auth::Permissions, error::AppError, state::AppState};
use axum::http::{HeaderMap, StatusCode, Uri};
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;

fn hmac_sha256(key: &[u8], msg: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts any key size");
    mac.update(msg);
    mac.finalize().into_bytes().to_vec()
}

fn sha256_hex(data: &[u8]) -> String {
    let hash = Sha256::digest(data);
    hash.iter().map(|b| format!("{b:02x}")).collect()
}

fn derive_signing_key(secret: &str, date: &str, region: &str, service: &str) -> Vec<u8> {
    let k_secret = format!("AWS4{secret}");
    let k_date = hmac_sha256(k_secret.as_bytes(), date.as_bytes());
    let k_region = hmac_sha256(&k_date, region.as_bytes());
    let k_service = hmac_sha256(&k_region, service.as_bytes());
    hmac_sha256(&k_service, b"aws4_request")
}

struct ParsedAuth {
    access_key_id: String,
    date: String,
    region: String,
    service: String,
    signed_headers: Vec<String>,
    signature: String,
}

fn parse_authorization(auth: &str) -> Option<ParsedAuth> {
    let rest = auth.strip_prefix("AWS4-HMAC-SHA256 ")?;

    let mut credential_str = None;
    let mut signed_headers_str = None;
    let mut signature = None;

    for part in rest.split(", ") {
        if let Some((k, v)) = part.split_once('=') {
            match k.trim() {
                "Credential" => credential_str = Some(v),
                "SignedHeaders" => signed_headers_str = Some(v),
                "Signature" => signature = Some(v.to_string()),
                _ => {}
            }
        }
    }

    let mut credential_parts = credential_str?.splitn(5, '/');
    let access_key_id = credential_parts.next()?.to_string();
    let date = credential_parts.next()?.to_string();
    let region = credential_parts.next()?.to_string();
    let service = credential_parts.next()?.to_string();
    // fifth segment is "aws4_request", not needed

    let signed_headers = signed_headers_str?
        .split(';')
        .map(str::to_lowercase)
        .collect();

    Some(ParsedAuth {
        access_key_id,
        date,
        region,
        service,
        signed_headers,
        signature: signature?,
    })
}

pub async fn verify_sigv4(
    state: &AppState,
    method: &str,
    uri: &Uri,
    headers: &HeaderMap,
    auth_header: &str,
) -> Result<Permissions, AppError> {
    let parsed = parse_authorization(auth_header)
        .ok_or_else(|| AppError::StatusCode(StatusCode::UNAUTHORIZED))?;

    let key_details = state
        .s3_key_manager
        .get_key(&parsed.access_key_id)
        .await
        .map_err(|_| AppError::StatusCode(StatusCode::UNAUTHORIZED))?;

    let datetime = headers
        .get("x-amz-date")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| AppError::StatusCode(StatusCode::UNAUTHORIZED))?;

    let content_sha256 = headers
        .get("x-amz-content-sha256")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("UNSIGNED-PAYLOAD");

    // Build canonical headers from the signed headers list (already sorted by the client)
    let mut canonical_headers = String::new();
    for name in &parsed.signed_headers {
        let value = headers
            .get(name.as_str())
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .trim()
            .to_string();
        canonical_headers.push_str(&format!("{name}:{value}\n"));
    }
    let signed_headers_str = parsed.signed_headers.join(";");

    // Build sorted canonical query string
    let canonical_qs = {
        let qs = uri.query().unwrap_or("");
        if qs.is_empty() {
            String::new()
        } else {
            let mut pairs: Vec<(&str, &str)> = qs
                .split('&')
                .filter_map(|p| p.split_once('=').or(Some((p, ""))))
                .collect();
            pairs.sort_by_key(|(k, _)| *k);
            pairs
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join("&")
        }
    };

    let canonical_request = format!(
        "{method}\n{}\n{canonical_qs}\n{canonical_headers}\n{signed_headers_str}\n{content_sha256}",
        uri.path(),
    );

    let scope = format!(
        "{}/{}/{}/aws4_request",
        parsed.date, parsed.region, parsed.service
    );
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{datetime}\n{scope}\n{}",
        sha256_hex(canonical_request.as_bytes())
    );

    let signing_key = derive_signing_key(
        &key_details.secret_access_key,
        &parsed.date,
        &parsed.region,
        &parsed.service,
    );
    let computed_sig: String = hmac_sha256(&signing_key, string_to_sign.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();

    if computed_sig != parsed.signature {
        return Err(AppError::StatusCode(StatusCode::UNAUTHORIZED));
    }

    Ok(Permissions {
        permitted_methods: key_details.permitted_methods,
        permitted_namespaces: key_details.permitted_namespaces,
        issuer: parsed.access_key_id,
    })
}
