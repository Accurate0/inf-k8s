use aws_config::SdkConfig;
use aws_sdk_dynamodb::{
    error::SdkError,
    operation::{get_item::GetItemError, put_item::PutItemError},
    types::AttributeValue,
};
use chrono::{DateTime, Utc};

#[derive(thiserror::Error, Debug)]
pub enum KeyManagerError {
    #[error("error adding key: {0}")]
    AddKey(#[from] SdkError<PutItemError>),
    #[error("error getting key: {0}")]
    GetKey(#[from] SdkError<GetItemError>),
    #[error("requested key not found: {0}")]
    KeyNotFound(String),
    #[error("key missing detail field: {0}")]
    MissingKeyDetail(&'static str),
    #[error("key detail field incorrect type: {0}")]
    TypeMismatch(&'static str),
    #[error("datetime parse error: {0}")]
    DateTimeParse(#[from] chrono::ParseError),
    #[error("ttl parse error: {0}")]
    TtlParse(#[from] std::num::ParseIntError),
}

#[derive(Clone)]
pub struct KeyManager {
    db_client: aws_sdk_dynamodb::Client,
}

pub struct KeyDetails {
    pub key_id: String,
    pub public_key: String,
    pub permitted_namespaces: Vec<String>,
    pub permitted_methods: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub ttl: Option<i64>,
}

impl KeyManager {
    const TABLE_NAME: &str = "object-registry-keys";
    const KEY_ID: &str = "key_id";
    const PUBLIC_KEY: &str = "public_key";
    const PERMITTED_NAMESPACES: &str = "permitted_namespaces";
    const PERMITTED_METHODS: &str = "permitted_methods";
    const CREATED_AT: &str = "created_at";
    const TTL: &str = "ttl";

    pub fn new(sdk_config: &SdkConfig) -> Self {
        Self {
            db_client: aws_sdk_dynamodb::Client::new(sdk_config),
        }
    }

    pub async fn add_key(&self, key_details: KeyDetails) -> Result<(), KeyManagerError> {
        let now = chrono::Utc::now();

        let mut req = self
            .db_client
            .put_item()
            .table_name(Self::TABLE_NAME)
            .item(Self::KEY_ID, AttributeValue::S(key_details.key_id))
            .item(Self::PUBLIC_KEY, AttributeValue::S(key_details.public_key))
            .item(
                Self::PERMITTED_NAMESPACES,
                AttributeValue::Ss(key_details.permitted_namespaces),
            )
            .item(
                Self::PERMITTED_METHODS,
                AttributeValue::Ss(key_details.permitted_methods),
            )
            .item(Self::CREATED_AT, AttributeValue::S(now.to_rfc3339()));

        if let Some(ttl) = key_details.ttl {
            req = req.item(Self::TTL, AttributeValue::N(ttl.to_string()));
        }

        req.send().await?;

        Ok(())
    }

    fn get_required_string(
        item: &std::collections::HashMap<String, AttributeValue>,
        field: &'static str,
    ) -> Result<String, KeyManagerError> {
        item.get(field)
            .ok_or_else(|| KeyManagerError::MissingKeyDetail(field))?
            .as_s()
            .map(|s| s.to_string())
            .map_err(|_| KeyManagerError::TypeMismatch(field))
    }

    fn get_required_string_set(
        item: &std::collections::HashMap<String, AttributeValue>,
        field: &'static str,
    ) -> Result<Vec<String>, KeyManagerError> {
        item.get(field)
            .ok_or_else(|| KeyManagerError::MissingKeyDetail(field))?
            .as_ss()
            .map(|ss| ss.to_vec())
            .map_err(|_| KeyManagerError::TypeMismatch(field))
    }

    pub async fn get_key_details(&self, key_id: String) -> Result<KeyDetails, KeyManagerError> {
        let response = self
            .db_client
            .get_item()
            .table_name(Self::TABLE_NAME)
            .key("key_id", AttributeValue::S(key_id.to_owned()))
            .consistent_read(true)
            .send()
            .await?;

        if let Some(item) = response.item() {
            let key_id = Self::get_required_string(item, Self::KEY_ID)?;
            let public_key = Self::get_required_string(item, Self::PUBLIC_KEY)?;
            let permitted_namespaces =
                Self::get_required_string_set(item, Self::PERMITTED_NAMESPACES)?;
            let permitted_methods = Self::get_required_string_set(item, Self::PERMITTED_METHODS)?;
            let created_at_str = Self::get_required_string(item, Self::CREATED_AT)?;

            let created_at = DateTime::parse_from_rfc3339(&created_at_str)?.with_timezone(&Utc);

            // optional TTL numeric attribute
            let ttl = if let Some(av) = item.get(Self::TTL) {
                let s = av
                    .as_n()
                    .map_err(|_| KeyManagerError::TypeMismatch(Self::TTL))?;
                Some(s.parse::<i64>()?)
            } else {
                None
            };

            Ok(KeyDetails {
                key_id,
                public_key,
                permitted_namespaces,
                permitted_methods,
                created_at,
                ttl,
            })
        } else {
            Err(KeyManagerError::KeyNotFound(key_id))
        }
    }
}
