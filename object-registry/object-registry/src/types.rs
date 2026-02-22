use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Deserialize, Serialize, Debug)]
pub struct NotifyRequest {
    #[serde(rename = "type")]
    pub r#type: String,
    pub method: String,
    pub urls: Vec<String>,
}

#[derive(Deserialize, Serialize, Debug)]
pub struct EventRequest {
    pub keys: Vec<String>,
    pub notify: NotifyRequest,
    pub audience: String,
    pub created_at: Option<String>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct CreatedResponse {
    pub id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct NotifyResponse {
    #[serde(rename = "type")]
    pub r#type: String,
    pub method: String,
    pub urls: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct MetadataResponse {
    pub namespace: String,
    pub checksum: String,
    pub size: usize,
    pub content_type: String,
    pub created_by: String,
    pub created_at: String,
    pub labels: HashMap<String, String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ObjectEvent {
    pub key: String,
    pub metadata: MetadataResponse,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ObjectMetadata {
    pub key: String,
    pub metadata: MetadataResponse,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ListObjectsResponse {
    pub objects: Vec<ObjectMetadata>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum OptionalObjectResponse<T> {
    ExistingObjectIsValid,
    ObjectUpdated(ObjectResponse<T>),
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ObjectResponse<T> {
    pub key: String,
    #[serde(default)]
    pub is_base64_encoded: bool,
    pub payload: T,
    pub metadata: MetadataResponse,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct EventResponse {
    pub namespace: String,
    pub id: String,
    pub keys: Vec<String>,
    pub notify: NotifyResponse,
    pub audience: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditLog {
    pub id: String,
    pub timestamp: i64,
    pub ttl: i64,
    pub action: String,
    pub subject: String,
    pub namespace: Option<String>,
    pub object_key: Option<String>,
    pub details: HashMap<String, String>,
}
