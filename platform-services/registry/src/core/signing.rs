use axum::{Json, extract::State};
use chrono::{Duration, Utc};
use registry_api::signing::{self, CertificateClaims, CertificateRequest, Envelope, PublicKey};
use serde_json::json;

use super::{
    audit::{self, Event},
    ensure_package_exists,
};
use crate::{
    auth::{Principal, policy::authorize_publish},
    error::{ApiError, Result},
    state::AppState,
};

pub const CERTIFICATE_LIFETIME_MINUTES: i64 = 15;

pub async fn keys(State(state): State<AppState>) -> Json<Vec<PublicKey>> {
    let key = state.signer.verifying_key();
    Json(vec![PublicKey {
        key_id: signing::key_id(&key),
        public_key_pem: signing::public_key_pem(&key),
    }])
}

pub async fn certificate(
    State(state): State<AppState>,
    principal: Principal,
    Json(request): Json<CertificateRequest>,
) -> Result<Json<Envelope>> {
    ensure_package_exists(&state.pool, &request.ecosystem, &request.package).await?;
    let publisher =
        authorize_publish(&state, &principal, &request.ecosystem, &request.package).await?;
    signing::decode_key(&request.public_key).map_err(ApiError::BadRequest)?;

    let issued_at = Utc::now();
    let claims = CertificateClaims {
        ecosystem: request.ecosystem.clone(),
        package: request.package.clone(),
        public_key: request.public_key,
        subject: publisher.actor.clone(),
        kind: publisher.kind.to_owned(),
        source: publisher.source,
        issued_at,
        expires_at: issued_at + Duration::minutes(CERTIFICATE_LIFETIME_MINUTES),
    };

    audit::record(
        &state.pool,
        &publisher.actor,
        publisher.kind,
        Event {
            action: "certificate.issue",
            ecosystem: Some(&request.ecosystem),
            package: Some(&request.package),
            version: None,
            detail: json!({ "key_id": signing::key_id(&signing::decode_key(&claims.public_key).map_err(ApiError::BadRequest)?) }),
        },
    )
    .await?;

    Ok(Json(signing::sign(state.signer.key(), &claims)))
}
