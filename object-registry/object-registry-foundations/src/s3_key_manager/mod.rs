use aws_config::SdkConfig;
use aws_sdk_dynamodb::{Client as DynamoClient, types::AttributeValue};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum S3KeyManagerError {
    #[error("key not found")]
    KeyNotFound,
    #[error("dynamo put error: {0}")]
    DynamoPut(#[from] aws_sdk_dynamodb::error::SdkError<aws_sdk_dynamodb::operation::put_item::PutItemError>),
    #[error("dynamo get error: {0}")]
    DynamoGet(#[from] aws_sdk_dynamodb::error::SdkError<aws_sdk_dynamodb::operation::get_item::GetItemError>),
}

pub struct S3KeyDetails {
    pub access_key_id: String,
    pub secret_access_key: String,
    pub permitted_namespaces: Vec<String>,
    pub permitted_methods: Vec<String>,
}

#[derive(Clone)]
pub struct S3KeyManager {
    dynamo_client: DynamoClient,
}

impl S3KeyManager {
    pub const TABLE_NAME: &str = "object-registry-s3-keys";
    const ACCESS_KEY_ID: &str = "access_key_id";
    const SECRET_ACCESS_KEY: &str = "secret_access_key";
    const PERMITTED_NAMESPACES: &str = "permitted_namespaces";
    const PERMITTED_METHODS: &str = "permitted_methods";

    pub fn new(config: &SdkConfig) -> Self {
        Self {
            dynamo_client: DynamoClient::new(config),
        }
    }

    pub async fn add_key(
        &self,
        access_key_id: &str,
        secret_access_key: &str,
        permitted_namespaces: Vec<String>,
        permitted_methods: Vec<String>,
    ) -> Result<(), S3KeyManagerError> {
        let namespaces = permitted_namespaces
            .into_iter()
            .map(AttributeValue::S)
            .collect();
        let methods = permitted_methods
            .into_iter()
            .map(AttributeValue::S)
            .collect();

        self.dynamo_client
            .put_item()
            .table_name(Self::TABLE_NAME)
            .item(Self::ACCESS_KEY_ID, AttributeValue::S(access_key_id.to_string()))
            .item(Self::SECRET_ACCESS_KEY, AttributeValue::S(secret_access_key.to_string()))
            .item(Self::PERMITTED_NAMESPACES, AttributeValue::L(namespaces))
            .item(Self::PERMITTED_METHODS, AttributeValue::L(methods))
            .send()
            .await?;

        Ok(())
    }

    pub async fn get_key(&self, access_key_id: &str) -> Result<S3KeyDetails, S3KeyManagerError> {
        let result = self
            .dynamo_client
            .get_item()
            .table_name(Self::TABLE_NAME)
            .key(Self::ACCESS_KEY_ID, AttributeValue::S(access_key_id.to_string()))
            .send()
            .await?;

        let item = result.item.ok_or(S3KeyManagerError::KeyNotFound)?;

        let secret_access_key = item
            .get(Self::SECRET_ACCESS_KEY)
            .and_then(|v| v.as_s().ok())
            .cloned()
            .ok_or(S3KeyManagerError::KeyNotFound)?;

        let permitted_namespaces = item
            .get(Self::PERMITTED_NAMESPACES)
            .and_then(|v| v.as_l().ok())
            .map(|l| {
                l.iter()
                    .filter_map(|v| v.as_s().ok().cloned())
                    .collect()
            })
            .unwrap_or_default();

        let permitted_methods = item
            .get(Self::PERMITTED_METHODS)
            .and_then(|v| v.as_l().ok())
            .map(|l| {
                l.iter()
                    .filter_map(|v| v.as_s().ok().cloned())
                    .collect()
            })
            .unwrap_or_default();

        Ok(S3KeyDetails {
            access_key_id: access_key_id.to_string(),
            secret_access_key,
            permitted_namespaces,
            permitted_methods,
        })
    }
}
