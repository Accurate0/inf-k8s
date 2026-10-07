use std::collections::BTreeMap;

use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use crate::{
    auth::AuthError,
    error::{GatewayError, Result},
    keys::UpdateKey,
    metrics, pricing,
    pricing::ModelPrice,
    state::AppState,
    usage,
    usage::Window,
};

#[allow(clippy::result_large_err)]
async fn authorize(state: &AppState, headers: &HeaderMap) -> std::result::Result<(), Response> {
    let provided = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim)
        .unwrap_or("");

    let denied = || (StatusCode::UNAUTHORIZED, "admin credentials required").into_response();

    if provided.is_empty() {
        return Err(denied());
    }

    if !state.config.admin_token.is_empty() && provided == state.config.admin_token {
        return Ok(());
    }

    let Some(auth) = &state.auth else {
        return Err(denied());
    };

    match auth.authorize(provided).await {
        Ok(claims) => {
            tracing::info!(
                actor = claims.identity(),
                "admin request authorized via oidc"
            );

            Ok(())
        }
        Err(AuthError::Unauthorized(reason)) => {
            tracing::warn!("admin bearer rejected: {reason}");

            Err(denied())
        }
        Err(AuthError::Forbidden(reason)) => {
            tracing::warn!("admin bearer forbidden: {reason}");

            Err((StatusCode::FORBIDDEN, "not a member of an admin group").into_response())
        }
        Err(e) => {
            tracing::error!("admin authentication failed: {e}");

            Err((
                StatusCode::SERVICE_UNAVAILABLE,
                "authentication unavailable",
            )
                .into_response())
        }
    }
}

pub async fn metrics_handler() -> impl IntoResponse {
    metrics::render()
}

pub async fn list_models(State(state): State<AppState>) -> impl IntoResponse {
    let mut models: BTreeMap<String, String> = state.providers.models().into_iter().collect();
    state.config.advertise(&mut models);

    let data: Vec<_> = models
        .into_iter()
        .map(|(id, provider)| {
            json!({ "id": id, "object": "model", "created": 0, "owned_by": provider })
        })
        .collect();
    Json(json!({ "object": "list", "data": data }))
}

#[derive(Deserialize)]
pub struct CreateKey {
    pub name: String,
    #[serde(default)]
    pub allowed_models: Vec<String>,
    #[serde(default)]
    pub monthly_token_budget: Option<i64>,
}

pub async fn create_key(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CreateKey>,
) -> Result<Response> {
    if let Err(resp) = authorize(&state, &headers).await {
        return Ok(resp);
    }

    let (token, info) = state
        .keys
        .create(&body.name, &body.allowed_models, body.monthly_token_budget)
        .await?;

    // The plaintext token is returned exactly once, here.
    Ok((
        StatusCode::CREATED,
        Json(json!({ "key": token, "info": info })),
    )
        .into_response())
}

pub async fn list_keys(State(state): State<AppState>, headers: HeaderMap) -> Result<Response> {
    if let Err(resp) = authorize(&state, &headers).await {
        return Ok(resp);
    }
    Ok(Json(state.keys.list().await?).into_response())
}

pub async fn revoke_key(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    if let Err(resp) = authorize(&state, &headers).await {
        return Ok(resp);
    }
    let found = state.keys.revoke(id).await?;
    Ok(if found {
        StatusCode::NO_CONTENT.into_response()
    } else {
        StatusCode::NOT_FOUND.into_response()
    })
}

pub async fn regenerate_key(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    if let Err(resp) = authorize(&state, &headers).await {
        return Ok(resp);
    }
    Ok(match state.keys.regenerate(id).await? {
        // The plaintext token is returned exactly once, here.
        Some((token, info)) => Json(json!({ "key": token, "info": info })).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    })
}

pub async fn update_key(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateKey>,
) -> Result<Response> {
    if let Err(resp) = authorize(&state, &headers).await {
        return Ok(resp);
    }
    Ok(match state.keys.update(id, &body).await? {
        Some(info) => Json(info).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    })
}

#[derive(Deserialize)]
pub struct UsageQuery {
    pub since: Option<String>,
}

pub async fn usage_summary(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<UsageQuery>,
) -> Result<Response> {
    if let Err(resp) = authorize(&state, &headers).await {
        return Ok(resp);
    }

    let window = match query.since.as_deref() {
        Some(since) => since.parse().map_err(GatewayError::BadRequest)?,
        None => Window::default(),
    };

    Ok(Json(usage::summary(&state.pool, window).await?).into_response())
}

pub async fn sync_prices(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(prices): Json<Vec<ModelPrice>>,
) -> Result<Response> {
    if let Err(resp) = authorize(&state, &headers).await {
        return Ok(resp);
    }
    let written = pricing::upsert(&state.pool, &prices).await?;
    state.pricing.refresh(&state.pool).await?;
    Ok(Json(json!({ "upserted": written })).into_response())
}
