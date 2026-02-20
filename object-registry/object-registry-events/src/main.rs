use aws_lambda_events::event::s3::S3Event;
use lambda_runtime::{Error, LambdaEvent, run, service_fn, tracing};
use object_registry::types::{MetadataResponse, ObjectEvent};
use object_registry_foundations::audit_manager::AuditManager;
use object_registry_foundations::event_manager::{EventManager, NotificationType};
use object_registry_foundations::generate_jwt_from_private_key;
use object_registry_foundations::object_manager::ObjectManager;
use reqwest::Method;
use std::collections::HashMap;
use std::str::FromStr;
use urlencoding::decode;

async fn s3_event_handler(event: LambdaEvent<S3Event>) -> Result<(), Error> {
    let config = aws_config::load_from_env().await;
    let secrets_client = aws_sdk_secretsmanager::Client::new(&config);
    let http_client = reqwest::ClientBuilder::new().build()?;
    let event_manager = EventManager::new(&config);
    let object_manager = ObjectManager::new(&config);
    let audit_manager = AuditManager::new(&config);

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
        let bucket_key = decode(&bucket_key)?.to_string();
        let mut values: Vec<&str> = bucket_key.splitn(2, '/').collect();
        let key = values.pop().unwrap().to_owned();
        let namespace = values.pop().unwrap();
        let _bucket = bucket.unwrap();

        tracing::info!("{namespace} {key} in bucket {_bucket}");

        let stored_object = match object_manager.get_object_by_key(&bucket_key).await {
            Ok(o) => o,
            Err(e) => {
                tracing::error!("error fetching object {bucket_key}: {e}");
                continue;
            }
        };

        let meta = MetadataResponse {
            namespace: stored_object.metadata.namespace,
            checksum: stored_object.metadata.checksum,
            size: stored_object.metadata.size,
            content_type: stored_object.metadata.content_type,
            created_by: stored_object.metadata.created_by,
            created_at: stored_object.metadata.created_at,
            labels: stored_object.metadata.labels,
        };

        let payload = ObjectEvent {
            key: key.clone(),
            metadata: meta,
        };

        let events = event_manager.get_events(namespace.to_string()).await?;
        for event in events {
            if (event.keys.contains(&"*".to_owned()) || event.keys.contains(&key))
                && event.namespace == namespace
            {
                tracing::info!("match in config for {event:?}");

                if event.notify.r#type == NotificationType::HTTP {
                    let method_str = &event.notify.method;
                    let urls = &event.notify.urls;
                    tracing::info!("sending http request: {method_str} to {urls:?}");
                    let method = Method::from_str(method_str)?;

                    let auth_token = generate_jwt_from_private_key(
                        jwt_secret.as_bytes(),
                        "object-registry",
                        &event.audience,
                    )?;

                    for url in urls {
                        let response = http_client
                            .request(method.clone(), url)
                            .bearer_auth(&auth_token)
                            .json(&payload)
                            .send()
                            .await?;

                        let status = response.status();
                        tracing::info!("response: {response:?}");
                        let body = response.text().await?;
                        tracing::info!("body: {}", body);

                        let mut details = HashMap::new();
                        details.insert("event_id".to_string(), event.id.clone());
                        details.insert("url".to_string(), url.clone());
                        details.insert("status".to_string(), status.to_string());
                        details.insert("method".to_string(), method_str.clone());
                        details.insert("audience".to_string(), event.audience.clone());

                        let _ = audit_manager
                            .log(
                                "EVENT_NOTIFY",
                                "object-registry-events",
                                Some(namespace),
                                Some(&key),
                                details,
                            )
                            .await;
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
