use crate::{error::AppError, state::AppState};
use axum::{
    extract::{Extension, Query, State},
    response::Response,
    http::StatusCode,
};
use serde::Deserialize;

#[derive(Deserialize)]
pub struct AuditQuery {
    pub limit: Option<i32>,
}

pub async fn list_audit_logs(
    State(state): State<AppState>,
    Extension(perms): Extension<crate::auth::Permissions>,
    Query(query): Query<AuditQuery>,
) -> Result<Response, AppError> {
    state
        .permissions_manager
        .enforce(&perms, "audit:list", "*")?;

    let limit = query.limit.unwrap_or(50);
    let logs = state.audit_manager.get_logs(limit).await
        .map_err(|_| AppError::StatusCode(StatusCode::INTERNAL_SERVER_ERROR))?;

    Ok(Response::builder()
        .status(200)
        .header("Content-Type", "application/json")
        .body(serde_json::to_string(&logs)?.into())?)
}
