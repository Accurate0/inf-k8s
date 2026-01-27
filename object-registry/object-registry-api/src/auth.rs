use crate::{error::AppError, state::AppState};
use axum::{
    extract::{MatchedPath, Request, State},
    http::{HeaderMap, Method, StatusCode},
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
    matched_path: Option<MatchedPath>,
    mut request: Request,
    next: Next,
) -> Result<Response, AppError> {
    if let Some(path) = matched_path {
        let path = path.as_str();
        tracing::info!("request matched path: {}", path);

        if path == "/health" || path == "/.well-known/jwks" {
            return Ok(next.run(request).await);
        }

        if path == "/{namespace}/public/{object}" && request.method() == Method::GET {
            return Ok(next.run(request).await);
        }
    }

    let Some(auth_header) = headers.get("Authorization") else {
        return Err(AppError::StatusCode(StatusCode::UNAUTHORIZED));
    };

    let token = auth_header
        .to_str()?
        .trim_start_matches("Bearer ")
        .to_string();

    tracing::info!("validating token");

    let header = decode_header(&token)?;
    let kid = header
        .kid
        .ok_or_else(|| AppError::StatusCode(StatusCode::UNAUTHORIZED))?;

    let key_details = app_state.key_manager.get_key_details(kid.clone()).await?;

    let public_pem = key_details.public_key;

    let decoding_key = DecodingKey::from_rsa_pem(public_pem.as_bytes())?;
    let mut validation = Validation::new(Algorithm::RS256);
    validation.set_audience(&["object-registry"]);
    validation.validate_exp = true;

    let token_data = match decode::<ObjectRegistryJwtClaims>(&token, &decoding_key, &validation) {
        Ok(td) => td,
        Err(e) => {
            tracing::error!("JWT decode failed for kid {}: {}", kid, e);
            return Err(AppError::StatusCode(StatusCode::UNAUTHORIZED));
        }
    };
    tracing::info!("verified request with claims: {:#?}", token_data.claims);

    let perms = Permissions {
        permitted_methods: key_details.permitted_methods.clone(),
        permitted_namespaces: key_details.permitted_namespaces.clone(),
    };
    request.extensions_mut().insert(perms);

    Ok(next.run(request).await)
}
