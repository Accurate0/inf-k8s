use aws_config::SdkConfig;
use aws_sdk_dynamodb::{types::AttributeValue, Client as DynamoClient};
use aws_sdk_s3::{
    error::SdkError,
    operation::{
        get_object::GetObjectError, list_objects_v2::ListObjectsV2Error,
        put_object::PutObjectError,
    },
    primitives::ByteStream,
};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ObjectManagerError {
    #[error("error putting object: {0}")]
    PutObject(#[from] SdkError<PutObjectError>),
    #[error("error getting object: {0}")]
    GetObject(#[from] SdkError<GetObjectError>),
    #[error("error listing objects: {0}")]
    ListObjects(#[from] SdkError<ListObjectsV2Error>),
    #[error("object not found")]
    ObjectNotFound,
    #[error("error reading object body: {0}")]
    ReadBody(#[from] aws_sdk_s3::primitives::ByteStreamError),
    #[error("error putting item to dynamodb: {0}")]
    DynamoPut(#[from] SdkError<aws_sdk_dynamodb::operation::put_item::PutItemError>),
    #[error("error getting item from dynamodb: {0}")]
    DynamoGet(#[from] SdkError<aws_sdk_dynamodb::operation::get_item::GetItemError>),
}

#[derive(Clone)]
pub struct ObjectManager {
    s3_client: aws_sdk_s3::Client,
    dynamo_client: DynamoClient,
}

use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct ObjectMetadata {
    pub namespace: String,
    pub checksum: String,
    pub size: usize,
    pub content_type: String,
    pub created_by: String,
    pub created_at: String,
    pub version: String,
    pub labels: HashMap<String, String>,
}

pub struct StoredObject {
    pub key: String,
    pub data: Vec<u8>,
    pub metadata: ObjectMetadata,
}

impl ObjectManager {
    pub const BUCKET_NAME: &str = "object-registry-inf-k8s";
    pub const TABLE_NAME: &str = "object-registry-metadata";

    pub const OBJECT_KEY: &str = "object_key";
    pub const NAMESPACE: &str = "namespace";
    pub const CHECKSUM: &str = "checksum";
    pub const SIZE: &str = "size";
    pub const CONTENT_TYPE: &str = "content_type";
    pub const CREATED_BY: &str = "created_by";
    pub const CREATED_AT: &str = "created_at";
    pub const VERSION: &str = "version";
    pub const LABELS: &str = "labels";

    pub fn new(config: &SdkConfig) -> Self {
        Self {
            s3_client: aws_sdk_s3::Client::new(config),
            dynamo_client: DynamoClient::new(config),
        }
    }

    fn get_key(namespace: &str, object: &str, version: Option<&str>, public: bool) -> String {
        if public {
            match version {
                Some(v) => format!("{namespace}/public/{object}@{v}"),
                None => format!("{namespace}/public/{object}"),
            }
        } else {
            match version {
                Some(v) => format!("{namespace}/{object}@{v}"),
                None => format!("{namespace}/{object}"),
            }
        }
    }

    pub async fn put_object(
        &self,
        namespace: &str,
        object: &str,
        version: Option<&str>,
        public: bool,
        body: Vec<u8>,
        content_type: &str,
        created_by: &str,
        labels: HashMap<String, String>,
    ) -> Result<String, ObjectManagerError> {
        let key = Self::get_key(namespace, object, version, public);

        let mut hasher = Sha256::new();
        hasher.update(&body);
        let checksum = hex::encode(hasher.finalize());
        let size = body.len();
        let created_at = chrono::Utc::now().to_rfc3339();

        self.s3_client
            .put_object()
            .bucket(Self::BUCKET_NAME)
            .key(&key)
            .body(ByteStream::from(body))
            .send()
            .await?;

        let labels_attr: HashMap<String, AttributeValue> = labels
            .into_iter()
            .map(|(k, v)| (k, AttributeValue::S(v)))
            .collect();

        self.dynamo_client
            .put_item()
            .table_name(Self::TABLE_NAME)
            .item(Self::OBJECT_KEY, AttributeValue::S(key.clone()))
            .item(Self::NAMESPACE, AttributeValue::S(namespace.to_string()))
            .item(Self::CHECKSUM, AttributeValue::S(checksum))
            .item(Self::SIZE, AttributeValue::N(size.to_string()))
            .item(Self::CONTENT_TYPE, AttributeValue::S(content_type.to_string()))
            .item(Self::CREATED_BY, AttributeValue::S(created_by.to_string()))
            .item(Self::CREATED_AT, AttributeValue::S(created_at))
            .item(
                Self::VERSION,
                AttributeValue::S(version.unwrap_or("latest").to_string()),
            )
            .item(Self::LABELS, AttributeValue::M(labels_attr))
            .send()
            .await?;

        Ok(key)
    }

    pub async fn list_objects(&self, prefix: &str) -> Result<Vec<String>, ObjectManagerError> {
        let list_output = self
            .s3_client
            .list_objects_v2()
            .bucket(Self::BUCKET_NAME)
            .prefix(prefix)
            .send()
            .await?;

        let mut keys = Vec::new();
        if let Some(objects) = list_output.contents {
            for object in objects {
                if let Some(key) = object.key {
                    keys.push(key);
                }
            }
        }
        Ok(keys)
    }

    pub async fn get_object(
        &self,
        namespace: &str,
        object: &str,
        version: Option<&str>,
        public: bool,
    ) -> Result<StoredObject, ObjectManagerError> {
        let key = Self::get_key(namespace, object, version, public);
        self.get_object_by_key(&key).await
    }

    pub async fn get_object_by_key(&self, key: &str) -> Result<StoredObject, ObjectManagerError> {
        let s3_result = self
            .s3_client
            .get_object()
            .bucket(Self::BUCKET_NAME)
            .key(key)
            .send()
            .await;

        let output = match s3_result {
            Ok(o) => o,
            Err(e) => {
                if let SdkError::ServiceError(err) = &e
                    && matches!(err.err(), GetObjectError::NoSuchKey(_)) {
                        return Err(ObjectManagerError::ObjectNotFound);
                    }
                return Err(ObjectManagerError::GetObject(e));
            }
        };

        let db_result = self
            .dynamo_client
            .get_item()
            .table_name(Self::TABLE_NAME)
            .key(Self::OBJECT_KEY, AttributeValue::S(key.to_string()))
            .send()
            .await?;

        let item = db_result.item.ok_or(ObjectManagerError::ObjectNotFound)?;

        let metadata = ObjectMetadata {
            namespace: item
                .get(Self::NAMESPACE)
                .and_then(|v| v.as_s().ok())
                .cloned()
                .unwrap_or_default(),
            checksum: item
                .get(Self::CHECKSUM)
                .and_then(|v| v.as_s().ok())
                .cloned()
                .unwrap_or_default(),
            size: item
                .get(Self::SIZE)
                .and_then(|v| v.as_n().ok())
                .and_then(|s| s.parse().ok())
                .unwrap_or_default(),
            content_type: item
                .get(Self::CONTENT_TYPE)
                .and_then(|v| v.as_s().ok())
                .cloned()
                .unwrap_or_default(),
            created_by: item
                .get(Self::CREATED_BY)
                .and_then(|v| v.as_s().ok())
                .cloned()
                .unwrap_or_default(),
            created_at: item
                .get(Self::CREATED_AT)
                .and_then(|v| v.as_s().ok())
                .cloned()
                .unwrap_or_default(),
            version: item
                .get(Self::VERSION)
                .and_then(|v| v.as_s().ok())
                .cloned()
                .unwrap_or_default(),
            labels: item
                .get(Self::LABELS)
                .and_then(|v| v.as_m().ok())
                .map(|m| {
                    m.iter()
                        .filter_map(|(k, v)| v.as_s().ok().map(|s| (k.clone(), s.clone())))
                        .collect()
                })
                .unwrap_or_default(),
        };

        let data = output.body.collect().await?.to_vec();
        Ok(StoredObject {
            key: key.to_string(),
            data,
            metadata,
        })
    }
}