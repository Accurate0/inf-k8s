use crate::{error::AppError, state::AppState};
use anyhow::Context;
use axum::{
    extract::{Request, State},
    http::{HeaderMap, StatusCode},
    middleware::Next,
    response::Response,
};
use lambda_http::tracing;
use object_registry::verify_jwt;

pub async fn auth_middleware(
    State(AppState { secrets_client, .. }): State<AppState>,
    headers: HeaderMap,
    request: Request,
    next: Next,
) -> Result<Response, AppError> {
    tracing::info!("request path: {}", request.uri().path());
    if request.uri().path() == "/health" {
        return Ok(next.run(request).await);
    }

    let Some(auth_header) = headers.get("Authorization") else {
        return Err(AppError::StatusCode(StatusCode::UNAUTHORIZED));
    };

    let auth_header_value = auth_header.to_str()?.replace("Bearer ", "");

    tracing::info!("validating {auth_header_value}");

    let jwt_secret = secrets_client
        .get_secret_value()
        .secret_id("config-catalog-jwt-secret")
        .send()
        .await?
        .secret_string
        .context("must have secret value")?;

    let claims = verify_jwt(jwt_secret.as_bytes(), &auth_header_value)?;

    tracing::info!("verified request with claims: {claims:?}");

    Ok(next.run(request).await)
}
