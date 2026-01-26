use aws_config::SdkConfig;
use aws_sdk_dynamodb::{
    error::SdkError,
    operation::{
        delete_item::DeleteItemError, get_item::GetItemError, put_item::PutItemError,
        query::QueryError,
    },
    types::AttributeValue,
};
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::fmt;

#[derive(thiserror::Error, Debug)]
pub enum EventManagerError {
    #[error("error adding event: {0}")]
    AddEvent(#[from] SdkError<PutItemError>),
    #[error("error getting event: {0}")]
    GetEvent(#[from] SdkError<GetItemError>),
    #[error("error querying events: {0}")]
    QueryEvent(#[from] SdkError<QueryError>),
    #[error("error deleting event: {0}")]
    DeleteEvent(#[from] SdkError<DeleteItemError>),
    #[error("requested event not found: {0}")]
    EventNotFound(String),
    #[error("event missing detail field: {0}")]
    MissingEventDetail(&'static str),
    #[error("event detail field incorrect type: {0}")]
    TypeMismatch(&'static str),
    #[error("datetime parse error: {0}")]
    DateTimeParse(#[from] chrono::ParseError),
}

#[derive(Clone)]
pub struct EventManager {
    db_client: aws_sdk_dynamodb::Client,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotificationType {
    HTTP,
    Unknown(String), // For handling types not explicitly defined
}

impl fmt::Display for NotificationType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            NotificationType::HTTP => write!(f, "HTTP"),
            NotificationType::Unknown(s) => write!(f, "{}", s),
        }
    }
}

impl From<String> for NotificationType {
    fn from(s: String) -> Self {
        match s.as_str() {
            "HTTP" => NotificationType::HTTP,
            _ => NotificationType::Unknown(s),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Notify {
    pub r#type: NotificationType,
    pub method: String,
    pub urls: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Event {
    pub namespace: String,
    pub id: String,
    pub keys: Vec<String>,
    pub notify: Notify,
    pub created_at: DateTime<Utc>,
}

impl EventManager {
    const TABLE_NAME: &str = "object-registry-events";
    const NAMESPACE: &str = "namespace";
    const ID: &str = "id";
    const KEYS: &str = "keys";
    const NOTIFY: &str = "notify";
    const NOTIFY_TYPE: &str = "type";
    const NOTIFY_METHOD: &str = "method";
    const NOTIFY_URLS: &str = "urls";
    const CREATED_AT: &str = "created_at";

    pub fn new(sdk_config: &SdkConfig) -> Self {
        Self {
            db_client: aws_sdk_dynamodb::Client::new(sdk_config),
        }
    }

    pub async fn add_event(&self, ev: Event) -> Result<(), EventManagerError> {
        let mut notify_map: HashMap<String, AttributeValue> = HashMap::new();
        notify_map.insert(Self::NOTIFY_TYPE.to_string(), AttributeValue::S(ev.notify.r#type.to_string()));
        notify_map.insert(Self::NOTIFY_METHOD.to_string(), AttributeValue::S(ev.notify.method));
        notify_map.insert(Self::NOTIFY_URLS.to_string(), AttributeValue::Ss(ev.notify.urls));

        self.db_client
            .put_item()
            .table_name(Self::TABLE_NAME)
            .item(Self::NAMESPACE, AttributeValue::S(ev.namespace))
            .item(Self::ID, AttributeValue::S(ev.id))
            .item(Self::KEYS, AttributeValue::Ss(ev.keys))
            .item(Self::NOTIFY, AttributeValue::M(notify_map))
            .item(Self::CREATED_AT, AttributeValue::S(Utc::now().to_rfc3339()))
            .send()
            .await?;

        Ok(())
    }

    /// Put (create or update) an event using the provided `ev`. This is idempotent
    /// with respect to `namespace`+`id` (it overwrites the item with the same key).
    pub async fn put_event(&self, ev: Event) -> Result<(), EventManagerError> {
        let mut notify_map: HashMap<String, AttributeValue> = HashMap::new();
        notify_map.insert(Self::NOTIFY_TYPE.to_string(), AttributeValue::S(ev.notify.r#type.to_string()));
        notify_map.insert(Self::NOTIFY_METHOD.to_string(), AttributeValue::S(ev.notify.method));
        notify_map.insert(Self::NOTIFY_URLS.to_string(), AttributeValue::Ss(ev.notify.urls));

        self.db_client
            .put_item()
            .table_name(Self::TABLE_NAME)
            .item(Self::NAMESPACE, AttributeValue::S(ev.namespace))
            .item(Self::ID, AttributeValue::S(ev.id))
            .item(Self::KEYS, AttributeValue::Ss(ev.keys))
            .item(Self::NOTIFY, AttributeValue::M(notify_map))
            .item(
                Self::CREATED_AT,
                AttributeValue::S(ev.created_at.to_rfc3339()),
            )
            .send()
            .await?;

        Ok(())
    }

    /// Delete an event by namespace and id
    pub async fn delete_event(
        &self,
        namespace: String,
        id: String,
    ) -> Result<(), EventManagerError> {
        self.db_client
            .delete_item()
            .table_name(Self::TABLE_NAME)
            .key(Self::NAMESPACE, AttributeValue::S(namespace))
            .key(Self::ID, AttributeValue::S(id))
            .send()
            .await?;

        Ok(())
    }

    fn get_required_string(
        item: &std::collections::HashMap<String, AttributeValue>,
        field: &'static str,
    ) -> Result<String, EventManagerError> {
        item.get(field)
            .ok_or_else(|| EventManagerError::MissingEventDetail(field))?
            .as_s()
            .map(|s| s.to_string())
            .map_err(|_| EventManagerError::TypeMismatch(field))
    }

    fn get_required_string_set(
        item: &std::collections::HashMap<String, AttributeValue>,
        field: &'static str,
    ) -> Result<Vec<String>, EventManagerError> {
        item.get(field)
            .ok_or_else(|| EventManagerError::MissingEventDetail(field))?
            .as_ss()
            .map(|ss| ss.to_vec())
            .map_err(|_| EventManagerError::TypeMismatch(field))
    }

    fn parse_notify(
        notify_attr: &HashMap<String, AttributeValue>,
    ) -> Result<Notify, EventManagerError> {
        let r#type = notify_attr
            .get(Self::NOTIFY_TYPE)
            .ok_or_else(|| EventManagerError::MissingEventDetail("notify.type"))?
            .as_s()
            .map(|s| NotificationType::from(s.to_string()))
            .map_err(|_| EventManagerError::TypeMismatch("notify.type"))?;

        let method = notify_attr
            .get(Self::NOTIFY_METHOD)
            .ok_or_else(|| EventManagerError::MissingEventDetail("notify.method"))?
            .as_s()
            .map(|s| s.to_string())
            .map_err(|_| EventManagerError::TypeMismatch("notify.method"))?;

        let urls = notify_attr
            .get(Self::NOTIFY_URLS)
            .ok_or_else(|| EventManagerError::MissingEventDetail("notify.urls"))?
            .as_ss()
            .map(|ss| ss.to_vec())
            .map_err(|_| EventManagerError::TypeMismatch("notify.urls"))?;

        Ok(Notify { r#type, method, urls })
    }

    pub async fn get_events(&self, namespace: String) -> Result<Vec<Event>, EventManagerError> {
        // Query by namespace (partition key) and return all matching items
        let response = self
            .db_client
            .query()
            .table_name(Self::TABLE_NAME)
            .key_condition_expression("namespace = :ns")
            .expression_attribute_values(":ns", AttributeValue::S(namespace))
            .send()
            .await?;

        let mut results = Vec::new();
        for item in response.items() {
            let ev = Self::parse_event(item)?;
            results.push(ev);
        }

        Ok(results)
    }

    fn parse_event(
        item: &std::collections::HashMap<String, AttributeValue>,
    ) -> Result<Event, EventManagerError> {
        let namespace = Self::get_required_string(item, Self::NAMESPACE)?;
        let id = Self::get_required_string(item, Self::ID)?;
        let keys = Self::get_required_string_set(item, Self::KEYS)?;

        let notify_attr = item
            .get(Self::NOTIFY)
            .ok_or_else(|| EventManagerError::MissingEventDetail(Self::NOTIFY))?
            .as_m()
            .map_err(|_| EventManagerError::TypeMismatch(Self::NOTIFY))?;

        let notify = Self::parse_notify(notify_attr)?;

        let created_at_str = Self::get_required_string(item, Self::CREATED_AT)?;
        let created_at = DateTime::parse_from_rfc3339(&created_at_str)?.with_timezone(&Utc);

        Ok(Event {
            namespace,
            id,
            keys,
            notify,
            created_at,
        })
    }
}
