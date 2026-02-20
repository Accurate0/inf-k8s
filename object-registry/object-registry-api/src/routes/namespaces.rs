use crate::{error::AppError, state::AppState};
use axum::{
    extract::{Extension, State},
    response::Response,
};
use std::collections::HashMap;

pub async fn list_namespaces(
    State(state): State<AppState>,
    Extension(perms): Extension<crate::auth::Permissions>,
) -> Result<Response, AppError> {
    state
        .permissions_manager
        .enforce(&perms, "namespace:list", "*")?;

    let audit_id = state
        .audit_manager
        .log("LIST_NAMESPACES", &perms.issuer, None, None, HashMap::new())
        .await?;

    let namespaces = state.object_manager.list_namespaces().await?;

    Ok(Response::builder()
        .status(200)
        .header("Content-Type", "application/json")
        .header(object_registry::X_AUDIT_ID_HEADER, audit_id.to_string())
        .body(serde_json::to_string(&namespaces)?.into())?)
}
