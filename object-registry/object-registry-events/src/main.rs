use aws_lambda_events::event::s3::S3Event;
use base64::{Engine, prelude::BASE64_STANDARD};
use lambda_runtime::{Error, LambdaEvent, run, service_fn, tracing};
use object_registry::event_manager::{EventManager, NotificationType};
use object_registry::generate_jwt_from_private_key;
use reqwest::{Method, header::CONTENT_TYPE};
use serde_json::{Value, json};
use std::str::FromStr;

#[derive(serde::Serialize)]
struct ConfigYamlEvent {
    pub key: String,
    pub payload: serde_yaml::Value,
}

async fn s3_event_handler(event: LambdaEvent<S3Event>) -> Result<(), Error> {
    let config = aws_config::load_from_env().await;
    let s3_client = aws_sdk_s3::Client::new(&config);
    let secrets_client = aws_sdk_secretsmanager::Client::new(&config);
    let http_client = reqwest::ClientBuilder::new().build()?;
    let event_manager = EventManager::new(&config);

    let jwt_secret = secrets_client
        .get_secret_value()
        .secret_id("object-registry-jwt-secret")
        .send()
        .await?
        .secret_string;

    if jwt_secret.is_none() {
        tracing::error!("no jwt secret found, cannot sign requests");
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

        let events = event_manager.get_events(namespace.to_string()).await?;
        for event in events {
            if (event.keys.contains(&"*".to_owned()) || event.keys.contains(&key))
                && event.namespace == namespace
            {
                tracing::info!("match in config for {event:?}");
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

                if event.notify.r#type == NotificationType::HTTP {
                    let method_str = &event.notify.method;
                    let urls = &event.notify.urls;
                    tracing::info!("sending http request: {method_str} to {urls:?}");
                    let method = Method::from_str(method_str)?;

                    let auth_token = generate_jwt_from_private_key(
                        jwt_secret.as_bytes(),
                        "object-registry",
                        &event.namespace,
                    )?;

                    for url in urls {
                        let response = http_client
                            .request(method.clone(), url)
                            .bearer_auth(&auth_token)
                            .header(CONTENT_TYPE, mime)
                            .body(payload.clone())
                            .send()
                            .await?;
                        tracing::info!("response: {response:?}");
                        tracing::info!("body: {}", response.text().await?);
                    }
                } else {
                    tracing::warn!("unsupported notification type: {}", event.notify.r#type);
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
