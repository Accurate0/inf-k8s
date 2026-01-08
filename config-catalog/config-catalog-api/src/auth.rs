use crate::{error::AppError, state::AppState};
use anyhow::Context;
use axum::{
    extract::{Request, State},
    http::{HeaderMap, StatusCode},
    middleware::Next,
    response::Response,
};
use config_catalog_jwt::{JwkSet, verify_github_actions_token, verify_jwt};
use lambda_http::tracing;

const GITHUB_ACTIONS_JWKS_URL: &str =
    "https://token.actions.githubusercontent.com/.well-known/jwks";

const ALLOWED_REPOS: [&str; 1] = ["Accurate0/home-gateway"];

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

    let is_from_github_actions = headers
        .get("X-Config-Catalog-Source")
        .map(|v| v.to_str().ok().map(|s| s == "github-actions"))
        .flatten()
        .unwrap_or(false);

    let auth_header_value = auth_header.to_str()?.replace("Bearer ", "");

    tracing::info!("validating {auth_header_value}");

    if is_from_github_actions {
        let jwks = reqwest::get(GITHUB_ACTIONS_JWKS_URL)
            .await?
            .error_for_status()?
            .json::<JwkSet>()
            .await?;

        let claims =
            verify_github_actions_token(jwks, &auth_header_value, ALLOWED_REPOS.as_slice())?;

        tracing::info!("verified gha request with claims: {claims:?}");
    } else {
        let jwt_secret = secrets_client
            .get_secret_value()
            .secret_id("config-catalog-jwt-secret")
            .send()
            .await?
            .secret_string
            .context("must have secret value")?;

        let claims = verify_jwt(jwt_secret.as_bytes(), &auth_header_value)?;

        tracing::info!("verified request with claims: {claims:?}");
    }

    Ok(next.run(request).await)
}
