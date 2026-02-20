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
    pub ttl: i64,
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
    const TTL: &str = "ttl";
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

    pub async fn get_latest_logs(
        &self,
        limit: i32,
        actions: Option<Vec<String>>,
        subjects: Option<Vec<String>>,
        namespaces: Option<Vec<String>>,
    ) -> Result<Vec<AuditLog>, AuditManagerError> {
        let mut all_logs = Vec::new();
        let mut last_evaluated_key = None;

        loop {
            let mut query = self
                .db_client
                .query()
                .table_name(Self::TABLE_NAME)
                .key_condition_expression("#pk = :pk")
                .expression_attribute_names("#pk", Self::PK)
                .expression_attribute_values(":pk", AttributeValue::S(Self::PK_VALUE.to_string()))
                .scan_index_forward(false) // Latest first
                .set_exclusive_start_key(last_evaluated_key);

            let mut filter_parts = Vec::new();

            if let Some(ref actions) = actions {
                if !actions.is_empty() {
                    let mut action_placeholders = Vec::new();
                    query = query.expression_attribute_names("#action", Self::ACTION);
                    for (i, action) in actions.iter().enumerate() {
                        let placeholder = format!(":action{}", i);
                        query = query.expression_attribute_values(placeholder.clone(), AttributeValue::S(action.clone()));
                        action_placeholders.push(placeholder);
                    }
                    filter_parts.push(format!("#action IN ({})", action_placeholders.join(", ")));
                }
            }

            if let Some(ref subjects) = subjects {
                if !subjects.is_empty() {
                    let mut subject_placeholders = Vec::new();
                    query = query.expression_attribute_names("#subject", Self::SUBJECT);
                    for (i, subject) in subjects.iter().enumerate() {
                        let placeholder = format!(":subject{}", i);
                        query = query.expression_attribute_values(placeholder.clone(), AttributeValue::S(subject.clone()));
                        subject_placeholders.push(placeholder);
                    }
                    filter_parts.push(format!("#subject IN ({})", subject_placeholders.join(", ")));
                }
            }

            if let Some(ref namespaces) = namespaces {
                if !namespaces.is_empty() {
                    let mut ns_placeholders = Vec::new();
                    query = query.expression_attribute_names("#namespace", Self::NAMESPACE);
                    for (i, ns) in namespaces.iter().enumerate() {
                        let placeholder = format!(":ns{}", i);
                        query = query.expression_attribute_values(placeholder.clone(), AttributeValue::S(ns.clone()));
                        ns_placeholders.push(placeholder);
                    }
                    filter_parts.push(format!("#namespace IN ({})", ns_placeholders.join(", ")));
                }
            }

            if !filter_parts.is_empty() {
                query = query.filter_expression(filter_parts.join(" AND "));
            } else {
                query = query.limit(limit - all_logs.len() as i32);
            }

            let db_result = query.send().await?;

            if let Some(items) = db_result.items {
                for item in items {
                    all_logs.push(self.map_item_to_audit_log(item));
                    if all_logs.len() >= limit as usize {
                        break;
                    }
                }
            }

            if all_logs.len() >= limit as usize {
                break;
            }

            last_evaluated_key = db_result.last_evaluated_key;
            if last_evaluated_key.is_none() {
                break;
            }
        }

        Ok(all_logs)
    }

    fn map_item_to_audit_log(&self, item: HashMap<String, AttributeValue>) -> AuditLog {
        AuditLog {
            id: item.get(Self::ID).and_then(|v| v.as_s().ok()).cloned().unwrap_or_default(),
            timestamp: item.get(Self::TIMESTAMP)
                .and_then(|v| v.as_n().ok())
                .and_then(|s| s.parse().ok())
                .unwrap_or_default(),
            ttl: item.get(Self::TTL)
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
        let now = Utc::now();
        let timestamp = now.timestamp_millis();
        let ttl = (now + chrono::Duration::days(14)).timestamp();
        let id = Uuid::new_v4().to_string();

        let mut item = HashMap::new();
        item.insert(Self::PK.to_string(), AttributeValue::S(Self::PK_VALUE.to_string()));
        item.insert(Self::ID.to_string(), AttributeValue::S(id));
        item.insert(Self::TIMESTAMP.to_string(), AttributeValue::N(timestamp.to_string()));
        item.insert(Self::TTL.to_string(), AttributeValue::N(ttl.to_string()));
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
