use crate::{error::AppError, state::AppState};
use axum::{
    extract::{Request, State},
    http::{HeaderMap, StatusCode},
    middleware::Next,
    response::Response,
};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use lambda_http::tracing;
use object_registry::ObjectRegistryJwtClaims;

#[derive(Clone)]
pub struct Permissions {
    pub permitted_methods: Vec<String>,
    pub permitted_namespaces: Vec<String>,
}

pub async fn auth_middleware(
    State(app_state): State<AppState>,
    headers: HeaderMap,
    mut request: Request,
    next: Next,
) -> Result<Response, AppError> {
    tracing::info!("request path: {}", request.uri().path());
    if request.uri().path() == "/health" {
        return Ok(next.run(request).await);
    }

    let Some(auth_header) = headers.get("Authorization") else {
        return Err(AppError::StatusCode(StatusCode::UNAUTHORIZED));
    };

    let token = auth_header
        .to_str()?
        .trim_start_matches("Bearer ")
        .to_string();

    tracing::info!("validating token");

    // Extract `kid` from token header
    let header = decode_header(&token)?;
    let kid = header
        .kid
        .ok_or_else(|| AppError::StatusCode(StatusCode::UNAUTHORIZED))?;

    // Lookup public key by kid
    let key_details = app_state.key_manager.get_key_details(kid.clone()).await?;

    let public_pem = key_details.public_key;

    // Build decoding key and validation
    let decoding_key = DecodingKey::from_rsa_pem(public_pem.as_bytes())?;
    let mut validation = Validation::new(Algorithm::RS256);
    // Only require the audience to be `object-registry`; do not enforce issuer.
    validation.set_audience(&["object-registry"]);
    validation.validate_exp = true;

    let token_data = decode::<ObjectRegistryJwtClaims>(&token, &decoding_key, &validation)?;
    tracing::info!("verified request with claims: {:#?}", token_data.claims);

    // Attach permitted methods/namespaces from key details into request extensions
    let perms = Permissions {
        permitted_methods: key_details.permitted_methods.clone(),
        permitted_namespaces: key_details.permitted_namespaces.clone(),
    };
    request.extensions_mut().insert(perms);

    Ok(next.run(request).await)
}
