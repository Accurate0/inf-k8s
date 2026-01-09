use crate::config::EventsConfig;
use aws_lambda_events::event::s3::S3Event;
use base64::{Engine, prelude::BASE64_STANDARD};
use config_catalog_jwt::generate_jwt;
use lambda_runtime::{Error, LambdaEvent, run, service_fn, tracing};
use reqwest::{
    Method,
    header::{AUTHORIZATION, CONTENT_TYPE},
};
use serde_json::{Value, json};
use std::str::FromStr;

mod config;

#[derive(serde::Serialize)]
struct ConfigYamlEvent {
    pub key: String,
    pub payload: serde_yaml::Value,
}

async fn s3_event_handler(event: LambdaEvent<S3Event>) -> Result<(), Error> {
    let events_config = EventsConfig::new()?;

    let config = aws_config::load_from_env().await;
    let s3_client = aws_sdk_s3::Client::new(&config);
    let secrets_client = aws_sdk_secretsmanager::Client::new(&config);
    let http_client = reqwest::ClientBuilder::new().build()?;

    let jwt_secret = secrets_client
        .get_secret_value()
        .secret_id("config-catalog-jwt-secret")
        .send()
        .await?
        .secret_string;

    if jwt_secret.is_none() {
        tracing::error!("no jwt secret found, cannot send requests");
        return Ok(());
    }

    let jwt_secret = jwt_secret.unwrap();

    for record in event.payload.records {
        let bucket_key = record.s3.object.key;
        let bucket = record.s3.bucket.name;

        if bucket_key.is_none() || bucket.is_none() {
            tracing::warn!("key or bucket is not named");
            continue;
        }

        let bucket_key = bucket_key.unwrap();
        let mut values: Vec<&str> = bucket_key.splitn(2, '/').collect();
        let key = values.pop().unwrap().to_owned();
        let namespace = values.pop().unwrap();
        let bucket = bucket.unwrap();

        tracing::info!("{namespace} {key} in bucket {bucket}");

        let stored_object = s3_client
            .get_object()
            .key(&bucket_key)
            .bucket(bucket)
            .send()
            .await?;

        let object_value = stored_object.body.collect().await?;
        let bytes = object_value.to_vec();

        for event_config in &events_config.events {
            let event_keys = &event_config.keys;
            let event_namespace = &event_config.namespace;
            if (event_keys.contains(&"*".to_owned()) || event_keys.contains(&key))
                && event_namespace == namespace
            {
                tracing::info!("match in config for {event_config:?}");
                let is_json_type = { serde_json::from_slice::<Value>(&bytes).is_ok() };
                let is_yaml_type = { serde_yaml::from_slice::<serde_yaml::Value>(&bytes).is_ok() };

                let (mime, payload) = if is_json_type {
                    (
                        "application/json",
                        json!({
                            "key": key,
                            "payload": serde_json::from_slice::<Value>(&bytes).unwrap()
                        })
                        .to_string(),
                    )
                } else if is_yaml_type {
                    (
                        "application/yaml",
                        serde_yaml::to_string(&ConfigYamlEvent {
                            key: key.clone(),
                            payload: serde_yaml::from_slice::<serde_yaml::Value>(&bytes).unwrap(),
                        })?,
                    )
                } else {
                    (
                        "application/json",
                        json!({ "key": key, "payload": BASE64_STANDARD.encode(bytes.clone()) })
                            .to_string(),
                    )
                };

                match event_config.notify {
                    config::Notify::HTTP {
                        ref method,
                        ref urls,
                        ref audience,
                    } => {
                        tracing::info!("sending http request: {method} to {urls:?}");
                        let method = Method::from_str(method)?;
                        let auth = generate_jwt(jwt_secret.as_bytes(), "config-catalog", audience)?;
                        for url in urls {
                            let response = http_client
                                .request(method.clone(), url)
                                .header(AUTHORIZATION, format!("Bearer {auth}"))
                                .header(CONTENT_TYPE, mime)
                                .body(payload.clone())
                                .send()
                                .await?;
                            tracing::info!("response: {response:?}");
                            tracing::info!("body: {}", response.text().await?);
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing::init_default_subscriber();
    run(service_fn(s3_event_handler)).await
}
