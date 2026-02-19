use crate::{error::AppError, state::AppState};
use axum::{
    extract::{Extension, State},
    response::Response,
};

pub async fn list_namespaces(
    State(state): State<AppState>,
    Extension(perms): Extension<crate::auth::Permissions>,
) -> Result<Response, AppError> {
    state
        .permissions_manager
        .enforce(&perms, "namespace:list", "*")?;

    let namespaces = state.object_manager.list_namespaces().await?;

    Ok(Response::builder()
        .status(200)
        .header("Content-Type", "application/json")
        .body(serde_json::to_string(&namespaces)?.into())?)
}
