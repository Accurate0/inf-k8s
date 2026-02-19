use axum::{
    extract::{Extension, Json, Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::auth::Permissions;
use crate::state::AppState;
use object_registry::event_manager::{Event, NotificationType, Notify};
use object_registry::types::{CreatedResponse, EventRequest, EventResponse};

pub async fn post_event(
    State(state): State<AppState>,
    Extension(perms): Extension<Permissions>,
    Path(namespace): Path<String>,
    Json(req): Json<EventRequest>,
) -> Result<impl IntoResponse, crate::error::AppError> {
    state
        .permissions_manager
        .enforce(&perms, "event:post", &namespace)?;

    let id = Uuid::new_v4().to_string();

    let created_at = if let Some(ts) = req.created_at {
        DateTime::parse_from_rfc3339(&ts)?.with_timezone(&Utc)
    } else {
        Utc::now()
    };
    let notify = Notify {
        r#type: NotificationType::from(req.notify.r#type),
        method: req.notify.method,
        urls: req.notify.urls,
    };

    let ev = Event {
        namespace: namespace.clone(),
        id: id.clone(),
        keys: req.keys,
        audience: req.audience,
        notify,
        created_at,
    };

    state.event_manager.put_event(ev).await?;

    Ok((StatusCode::CREATED, Json(CreatedResponse { id })))
}

pub async fn put_event(
    State(state): State<AppState>,
    Extension(perms): Extension<Permissions>,
    Path((namespace, id)): Path<(String, String)>,
    Json(req): Json<EventRequest>,
) -> Result<impl IntoResponse, crate::error::AppError> {
    state
        .permissions_manager
        .enforce(&perms, "event:put", &namespace)?;

    let created_at = if let Some(ts) = req.created_at {
        DateTime::parse_from_rfc3339(&ts)?.with_timezone(&Utc)
    } else {
        Utc::now()
    };

    let notify = Notify {
        r#type: NotificationType::from(req.notify.r#type),
        method: req.notify.method,
        urls: req.notify.urls,
    };

    let ev = Event {
        namespace: namespace.clone(),
        id: id.clone(),
        keys: req.keys,
        audience: req.audience,
        notify,
        created_at,
    };

    state.event_manager.put_event(ev).await?;

    Ok((StatusCode::OK, Json(CreatedResponse { id })))
}

pub async fn delete_event(
    State(state): State<AppState>,
    Extension(perms): Extension<Permissions>,
    Path((namespace, id)): Path<(String, String)>,
) -> Result<impl IntoResponse, crate::error::AppError> {
    state
        .permissions_manager
        .enforce(&perms, "event:delete", &namespace)?;
    state.event_manager.delete_event(id).await?;
    Ok((StatusCode::NO_CONTENT, ""))
}

pub async fn list_events(
    State(state): State<AppState>,
    Extension(perms): Extension<Permissions>,
    Path(namespace): Path<String>,
) -> Result<impl IntoResponse, crate::error::AppError> {
    state
        .permissions_manager
        .enforce(&perms, "event:get", &namespace)?;
    let evs = state.event_manager.get_events(namespace).await?;
    let arr: Vec<EventResponse> = evs.iter().map(EventResponse::from).collect();
    Ok((StatusCode::OK, Json(arr)))
}
