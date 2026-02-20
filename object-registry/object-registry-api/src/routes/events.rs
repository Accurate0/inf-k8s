use axum::{
    body::Body,
    extract::{Extension, Json, Path, State},
    http::StatusCode,
    response::Response,
};
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use uuid::Uuid;

use crate::auth::Permissions;
use crate::state::AppState;
use object_registry::types::{CreatedResponse, EventRequest, EventResponse};
use object_registry_foundations::event_manager::{Event, NotificationType, Notify};

pub async fn post_event(
    State(state): State<AppState>,
    Extension(perms): Extension<Permissions>,
    Path(namespace): Path<String>,
    Json(req): Json<EventRequest>,
) -> Result<Response, crate::error::AppError> {
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
        audience: req.audience.clone(),
        notify,
        created_at,
    };

    state.event_manager.put_event(ev).await?;

    let mut details = HashMap::new();
    details.insert("event_id".to_string(), id.clone());
    details.insert("audience".to_string(), req.audience);

    let audit_id = state
        .audit_manager
        .log("POST_EVENT", &perms.issuer, Some(&namespace), None, details)
        .await?;

    Ok(Response::builder()
        .status(StatusCode::CREATED)
        .header(object_registry::X_AUDIT_ID_HEADER, audit_id.to_string())
        .header("Content-Type", "application/json")
        .body(serde_json::to_string(&CreatedResponse { id })?.into())?)
}

pub async fn put_event(
    State(state): State<AppState>,
    Extension(perms): Extension<Permissions>,
    Path((namespace, id)): Path<(String, String)>,
    Json(req): Json<EventRequest>,
) -> Result<Response, crate::error::AppError> {
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
        audience: req.audience.clone(),
        notify,
        created_at,
    };

    state.event_manager.put_event(ev).await?;

    let mut details = HashMap::new();
    details.insert("event_id".to_string(), id.clone());
    details.insert("audience".to_string(), req.audience);

    let audit_id = state
        .audit_manager
        .log("PUT_EVENT", &perms.issuer, Some(&namespace), None, details)
        .await?;

    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(object_registry::X_AUDIT_ID_HEADER, audit_id.to_string())
        .header("Content-Type", "application/json")
        .body(serde_json::to_string(&CreatedResponse { id })?.into())?)
}

pub async fn delete_event(
    State(state): State<AppState>,
    Extension(perms): Extension<Permissions>,
    Path((namespace, id)): Path<(String, String)>,
) -> Result<Response, crate::error::AppError> {
    state
        .permissions_manager
        .enforce(&perms, "event:delete", &namespace)?;
    state.event_manager.delete_event(id.clone()).await?;

    let mut details = HashMap::new();
    details.insert("event_id".to_string(), id);

    let audit_id = state
        .audit_manager
        .log(
            "DELETE_EVENT",
            &perms.issuer,
            Some(&namespace),
            None,
            details,
        )
        .await?;

    Ok(Response::builder()
        .status(StatusCode::NO_CONTENT)
        .header(object_registry::X_AUDIT_ID_HEADER, audit_id.to_string())
        .body(Body::empty())?)
}

pub async fn list_events(
    State(state): State<AppState>,
    Extension(perms): Extension<Permissions>,
    Path(namespace): Path<String>,
) -> Result<Response, crate::error::AppError> {
    state
        .permissions_manager
        .enforce(&perms, "event:get", &namespace)?;

    let audit_id = state
        .audit_manager
        .log(
            "LIST_EVENTS",
            &perms.issuer,
            Some(&namespace),
            None,
            HashMap::new(),
        )
        .await?;

    let evs = state.event_manager.get_events(namespace).await?;
    let arr: Vec<EventResponse> = evs.iter().map(EventResponse::from).collect();

    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(object_registry::X_AUDIT_ID_HEADER, audit_id.to_string())
        .header("Content-Type", "application/json")
        .body(serde_json::to_string(&arr)?.into())?)
}
