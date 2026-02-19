use aws_config::SdkConfig;
use aws_sdk_dynamodb::{
    error::SdkError,
    operation::{put_item::PutItemError, query::QueryError, scan::ScanError},
    types::AttributeValue,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

#[derive(thiserror::Error, Debug)]
pub enum AuditManagerError {
    #[error("error adding audit log: {0}")]
    AddAuditLog(#[from] SdkError<PutItemError>),
    #[error("error scanning audit logs: {0}")]
    ScanAuditLogs(#[from] SdkError<ScanError>),
    #[error("error querying audit logs: {0}")]
    QueryAuditLogs(#[from] SdkError<QueryError>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditLog {
    pub id: String,
    pub timestamp: i64,
    pub action: String,
    pub subject: String,
    pub namespace: Option<String>,
    pub object_key: Option<String>,
    pub details: HashMap<String, String>,
}

#[derive(Clone)]
pub struct AuditManager {
    db_client: aws_sdk_dynamodb::Client,
}

impl AuditManager {
    const TABLE_NAME: &str = "object-registry-audit";
    const PK: &str = "pk";
    const PK_VALUE: &str = "AUDIT";
    const ID: &str = "id";
    const TIMESTAMP: &str = "timestamp";
    const ACTION: &str = "action";
    const SUBJECT: &str = "subject";
    const NAMESPACE: &str = "namespace";
    const OBJECT_KEY: &str = "object_key";
    const DETAILS: &str = "details";

    pub fn new(sdk_config: &SdkConfig) -> Self {
        Self {
            db_client: aws_sdk_dynamodb::Client::new(sdk_config),
        }
    }

    pub async fn get_latest_logs(&self, limit: i32) -> Result<Vec<AuditLog>, AuditManagerError> {
        let db_result = self
            .db_client
            .query()
            .table_name(Self::TABLE_NAME)
            .key_condition_expression("#pk = :pk")
            .expression_attribute_names("#pk", Self::PK)
            .expression_attribute_values(":pk", AttributeValue::S(Self::PK_VALUE.to_string()))
            .scan_index_forward(false) // Latest first
            .limit(limit)
            .send()
            .await?;

        let items = db_result.items.unwrap_or_default();
        Ok(items.into_iter().map(|item| self.map_item_to_audit_log(item)).collect())
    }

    fn map_item_to_audit_log(&self, item: HashMap<String, AttributeValue>) -> AuditLog {
        AuditLog {
            id: item.get(Self::ID).and_then(|v| v.as_s().ok()).cloned().unwrap_or_default(),
            timestamp: item.get(Self::TIMESTAMP)
                .and_then(|v| v.as_n().ok())
                .and_then(|s| s.parse().ok())
                .unwrap_or_default(),
            action: item.get(Self::ACTION).and_then(|v| v.as_s().ok()).cloned().unwrap_or_default(),
            subject: item.get(Self::SUBJECT).and_then(|v| v.as_s().ok()).cloned().unwrap_or_default(),
            namespace: item.get(Self::NAMESPACE).and_then(|v| v.as_s().ok()).cloned(),
            object_key: item.get(Self::OBJECT_KEY).and_then(|v| v.as_s().ok()).cloned(),
            details: item.get(Self::DETAILS).and_then(|v| v.as_m().ok()).map(|m| {
                m.iter().filter_map(|(k, v)| v.as_s().ok().map(|s| (k.clone(), s.clone()))).collect()
            }).unwrap_or_default(),
        }
    }

    pub async fn log(
        &self,
        action: &str,
        subject: &str,
        namespace: Option<&str>,
        object_key: Option<&str>,
        details: HashMap<String, String>,
    ) -> Result<(), AuditManagerError> {
        let timestamp = Utc::now().timestamp_millis();
        let id = Uuid::new_v4().to_string();

        let mut item = HashMap::new();
        item.insert(Self::PK.to_string(), AttributeValue::S(Self::PK_VALUE.to_string()));
        item.insert(Self::ID.to_string(), AttributeValue::S(id));
        item.insert(Self::TIMESTAMP.to_string(), AttributeValue::N(timestamp.to_string()));
        item.insert(Self::ACTION.to_string(), AttributeValue::S(action.to_string()));
        item.insert(Self::SUBJECT.to_string(), AttributeValue::S(subject.to_string()));

        if let Some(ns) = namespace {
            item.insert(Self::NAMESPACE.to_string(), AttributeValue::S(ns.to_string()));
        }

        if let Some(key) = object_key {
            item.insert(Self::OBJECT_KEY.to_string(), AttributeValue::S(key.to_string()));
        }

        if !details.is_empty() {
            let details_attr: HashMap<String, AttributeValue> = details
                .into_iter()
                .map(|(k, v)| (k, AttributeValue::S(v)))
                .collect();
            item.insert(Self::DETAILS.to_string(), AttributeValue::M(details_attr));
        }

        self.db_client
            .put_item()
            .table_name(Self::TABLE_NAME)
            .set_item(Some(item))
            .send()
            .await?;

        Ok(())
    }
}
