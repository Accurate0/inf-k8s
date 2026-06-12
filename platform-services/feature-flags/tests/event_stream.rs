mod common;

use common::{connect_admin, connect_eval, eval_request, spawn_server};
use feature_flags::convert;
use feature_flags::pb;
use serde_json::Value;
use sqlx::PgPool;

#[sqlx::test(migrations = "./migrations")]
#[serial_test::serial]
async fn stream_events_delivers_ready_then_change(pool: PgPool) {
    let (endpoint, server_handle) = spawn_server(pool).await;
    let mut admin_client = connect_admin(&endpoint).await;
    let mut eval_client = connect_eval(&endpoint).await;

    let mut stream = eval_client
        .stream_events(eval_request(pb::EventStreamRequest {}))
        .await
        .unwrap()
        .into_inner();

    let ready = stream.message().await.unwrap().unwrap();
    assert_eq!(ready.r#type, pb::EventType::Ready as i32);

    admin_client
        .create_flag(pb::CreateFlagRequest {
            key: "new-flag".into(),
            value_type: pb::ValueType::Boolean as i32,
            enabled: true,
            default_variant_key: "on".into(),
            variants: vec![pb::Variant {
                key: "on".into(),
                value: Some(convert::json_to_prost_value(&Value::Bool(true))),
            }],
        })
        .await
        .unwrap();

    let change = stream.message().await.unwrap().unwrap();
    assert_eq!(change.r#type, pb::EventType::ConfigurationChanged as i32);
    assert!(change.config_version > ready.config_version);
    assert_eq!(change.changed_flag_keys, vec!["new-flag"]);

    server_handle.abort();
}
