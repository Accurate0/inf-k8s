use crate::{error::AppError, state::AppState};
use axum::{
    extract::{Extension, RawQuery, State},
    response::Response,
    http::StatusCode,
};

pub async fn list_audit_logs(
    State(state): State<AppState>,
    Extension(perms): Extension<crate::auth::Permissions>,
    RawQuery(query_string): RawQuery,
) -> Result<Response, AppError> {
    state
        .permissions_manager
        .enforce(&perms, "audit:list", "*")?;

    let mut limit = 50;
    let mut actions = Vec::new();
    let mut subjects = Vec::new();
    let mut namespaces = Vec::new();

    if let Some(query_string) = query_string {
        for (k, v) in form_urlencoded::parse(query_string.as_bytes()) {
            match k.as_ref() {
                "limit" => {
                    if let Ok(l) = v.parse::<i32>() {
                        limit = l;
                    }
                }
                "action" => actions.push(v.into_owned()),
                "subject" => subjects.push(v.into_owned()),
                "namespace" => namespaces.push(v.into_owned()),
                _ => {}
            }
        }
    }

    let actions = if actions.is_empty() { None } else { Some(actions) };
    let subjects = if subjects.is_empty() { None } else { Some(subjects) };
    let namespaces = if namespaces.is_empty() { None } else { Some(namespaces) };

    let logs = state
        .audit_manager
        .get_latest_logs(limit, actions, subjects, namespaces)
        .await
        .map_err(|_| AppError::StatusCode(StatusCode::INTERNAL_SERVER_ERROR))?;

    Ok(Response::builder()
        .status(200)
        .header("Content-Type", "application/json")
        .body(serde_json::to_string(&logs)?.into())?)
}
