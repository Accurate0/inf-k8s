use axum::{Json, extract::State, http::StatusCode};
use chrono::{DateTime, Duration, Utc};
use registry_api::{GraphToken, GraphTokenRequest};
use serde::Deserialize;
use serde_json::json;

use super::audit::{self, Event};
use crate::{
    auth::{ForgejoClaims, Principal, oidc::unverified_claims},
    config::{GraphConfig, GraphExchangeConfig},
    error::{ApiError, Result},
    state::AppState,
};

const TOKEN_EXCHANGE: &str = "urn:ietf:params:oauth:grant-type:token-exchange";
const ACCESS_TOKEN: &str = "urn:ietf:params:oauth:token-type:access_token";

fn configured(state: &AppState) -> Result<(&GraphConfig, &GraphExchangeConfig)> {
    state
        .config
        .graph
        .as_ref()
        .and_then(|graph| graph.exchange.as_ref().map(|exchange| (graph, exchange)))
        .ok_or_else(|| {
            ApiError::BadRequest("the graph token exchange is not configured".to_owned())
        })
}

#[derive(Deserialize)]
struct Exchanged {
    access_token: String,
    expires_in: i64,
}

#[derive(Deserialize)]
struct Session {
    jti: String,
}

pub async fn create(
    State(state): State<AppState>,
    principal: Principal,
    Json(request): Json<GraphTokenRequest>,
) -> Result<Json<GraphToken>> {
    let (graph, exchange) = configured(&state)?;
    let Principal::Forgejo(claims) = &principal else {
        return Err(ApiError::Forbidden(
            "only forgejo actions tokens are exchanged for graph service accounts".to_owned(),
        ));
    };

    let account = exchange
        .accounts
        .iter()
        .find(|a| a.name == request.account)
        .ok_or_else(|| ApiError::NotFound(format!("no graph account {}", request.account)))?;
    let granted = exchange
        .grants
        .iter()
        .any(|g| g.accounts.contains(&account.name) && g.forgejo.matches(claims));
    if !granted {
        return Err(ApiError::Forbidden(format!(
            "{} (ref {}) may not use {}",
            claims.repository,
            claims.git_ref.as_deref().unwrap_or("?"),
            account.name
        )));
    }

    let subject = std::env::var(&account.token_env).map_err(|_| {
        ApiError::AuthUnavailable(format!(
            "{} is unset for {}",
            account.token_env, account.name
        ))
    })?;
    let scope = account.scopes.join(" ");
    let mut form = vec![
        ("grant_type", TOKEN_EXCHANGE),
        ("subject_token", subject.as_str()),
        ("subject_token_type", ACCESS_TOKEN),
        ("audience", graph.audience.as_str()),
        ("client_id", graph.audience.as_str()),
    ];
    if !scope.is_empty() {
        form.push(("scope", scope.as_str()));
    }

    let response = state
        .http
        .post(&exchange.token_url)
        .form(&form)
        .send()
        .await
        .map_err(|e| ApiError::AuthUnavailable(format!("kanidm token exchange: {e}")))?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(ApiError::AuthUnavailable(format!(
            "kanidm refused the token exchange for {} ({status}): {body}",
            account.name
        )));
    }
    let exchanged: Exchanged = response
        .json()
        .await
        .map_err(|e| ApiError::AuthUnavailable(format!("kanidm token exchange response: {e}")))?;
    let Session { jti } = unverified_claims(&exchanged.access_token).ok_or_else(|| {
        ApiError::AuthUnavailable("kanidm returned an access token without a jti".to_owned())
    })?;
    let expires_at = Utc::now() + Duration::seconds(exchanged.expires_in);
    let forgejo = serde_json::to_value(claims).unwrap_or_default();

    sqlx::query!(
        "INSERT INTO core_graph_exchanges (jti, account, actor, forgejo, expires_at)
         VALUES ($1, $2, $3, $4, $5)",
        jti,
        account.name,
        principal.actor(),
        forgejo,
        expires_at
    )
    .execute(&state.pool)
    .await?;

    audit::record_as(
        &state.pool,
        &principal,
        Event {
            action: "graph.exchange",
            ecosystem: None,
            package: None,
            version: None,
            detail: json!({
                "account": account.name,
                "id": jti,
                "run": claims.run_id,
                "expires_at": expires_at,
            }),
        },
    )
    .await?;
    tracing::info!(
        account = account.name,
        actor = principal.actor(),
        "issued graph token"
    );

    Ok(Json(GraphToken {
        id: jti,
        account: account.name.clone(),
        access_token: exchanged.access_token,
        expires_at,
    }))
}

#[derive(Deserialize)]
pub struct RevokeRequest {
    pub token: String,
}

pub async fn revoke(
    State(state): State<AppState>,
    principal: Principal,
    Json(request): Json<RevokeRequest>,
) -> Result<StatusCode> {
    let (graph, exchange) = configured(&state)?;
    let Session { jti } = unverified_claims(&request.token)
        .ok_or_else(|| ApiError::BadRequest("not a graph access token".to_owned()))?;

    let row = sqlx::query!(
        "SELECT actor FROM core_graph_exchanges WHERE jti = $1 AND revoked_at IS NULL",
        jti
    )
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| ApiError::NotFound(format!("no live graph token {jti}")))?;
    if row.actor != principal.actor() && !principal.is_admin() {
        return Err(ApiError::Forbidden(format!(
            "graph token {jti} was issued to {}",
            row.actor
        )));
    }

    let response = state
        .http
        .post(&exchange.revoke_url)
        .form(&[
            ("token", request.token.as_str()),
            ("client_id", graph.audience.as_str()),
        ])
        .send()
        .await
        .map_err(|e| ApiError::AuthUnavailable(format!("kanidm token revocation: {e}")))?;
    if !response.status().is_success() {
        return Err(ApiError::AuthUnavailable(format!(
            "kanidm refused to revoke {jti} ({})",
            response.status()
        )));
    }

    sqlx::query!(
        "UPDATE core_graph_exchanges SET revoked_at = now() WHERE jti = $1",
        jti
    )
    .execute(&state.pool)
    .await?;
    audit::record_as(
        &state.pool,
        &principal,
        Event {
            action: "graph.revoke",
            ecosystem: None,
            package: None,
            version: None,
            detail: json!({ "id": jti }),
        },
    )
    .await?;

    Ok(StatusCode::NO_CONTENT)
}

pub async fn exchanged_for(
    state: &AppState,
    account: &str,
    jti: &str,
) -> Result<Option<ForgejoClaims>> {
    let row = sqlx::query!(
        r#"SELECT forgejo, expires_at AS "expires_at: DateTime<Utc>" FROM core_graph_exchanges
           WHERE jti = $1 AND account = $2 AND revoked_at IS NULL"#,
        jti,
        account
    )
    .fetch_optional(&state.pool)
    .await?;

    Ok(row
        .filter(|row| row.expires_at > Utc::now())
        .and_then(|row| serde_json::from_value(row.forgejo).ok()))
}

pub async fn collect_expired(state: &AppState) -> Result<u64> {
    let removed = sqlx::query!(
        "DELETE FROM core_graph_exchanges WHERE expires_at < now() - interval '1 day'"
    )
    .execute(&state.pool)
    .await?
    .rows_affected();
    Ok(removed)
}
