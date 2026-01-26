use serde::{Deserialize, Serialize};

/// Types used by the HTTP API layer for events (requests and responses).
#[derive(Deserialize, Serialize, Debug)]
pub struct NotifyRequest {
    #[serde(rename = "type")]
    pub typ: String,
    pub method: String,
    pub audience: String,
    pub urls: Vec<String>,
}

#[derive(Deserialize, Serialize, Debug)]
pub struct EventRequest {
    pub keys: Vec<String>,
    pub notify: NotifyRequest,
    pub created_at: Option<String>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct CreatedResponse {
    pub id: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct NotifyResponse {
    #[serde(rename = "type")]
    pub typ: String,
    pub method: String,
    pub audience: String,
    pub urls: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct EventResponse {
    pub namespace: String,
    pub id: String,
    pub keys: Vec<String>,
    pub notify: NotifyResponse,
    pub created_at: String,
}

impl From<&crate::event_manager::Event> for EventResponse {
    fn from(ev: &crate::event_manager::Event) -> Self {
        EventResponse {
            namespace: ev.namespace.clone(),
            id: ev.id.clone(),
            keys: ev.keys.clone(),
            notify: NotifyResponse {
                typ: ev.notify.typ.clone(),
                method: ev.notify.method.clone(),
                audience: ev.notify.audience.clone(),
                urls: ev.notify.urls.clone(),
            },
            created_at: ev.created_at.to_rfc3339(),
        }
    }
}
