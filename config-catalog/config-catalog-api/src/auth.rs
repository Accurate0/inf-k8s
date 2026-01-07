use crate::state::AppState;
use axum::{
    extract::{Request, State},
    http::{HeaderMap, StatusCode},
    middleware::Next,
    response::Response,
};
use jwt_base::verify_jwt;
use lambda_http::tracing;

pub async fn auth_middleware(
    State(AppState { secrets_client, .. }): State<AppState>,
    headers: HeaderMap,
    mut request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let Some(auth_header) = headers.get("Authorization") else {
        return Err(StatusCode::UNAUTHORIZED);
    };

    let jwt_secret = secrets_client
        .get_secret_value()
        .secret_id("config-catalog-jwt-secret")
        .send()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .secret_string
        .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;

    let auth_header_value = auth_header
        .to_str()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let claims = verify_jwt(jwt_secret.as_bytes(), auth_header_value)
        .map_err(|_| StatusCode::UNAUTHORIZED)?;

    tracing::info!("verified requests with claims: {claims:?}");
    request.extensions_mut().insert(claims);

    Ok(next.run(request).await)
}
