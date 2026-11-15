use insta::assert_yaml_snapshot;
use serde_json::{Value, json};

use crate::setup::Stack;

const SCHEMA: &str = "{ __schema { types { name fields { name } } } }";

fn root_fields(response: &Value) -> Value {
    let types = response["data"]["__schema"]["types"].as_array().unwrap();
    let names = |root: &str| {
        types.iter().find(|ty| ty["name"] == root).unwrap()["fields"]
            .as_array()
            .unwrap()
            .iter()
            .map(|field| field["name"].clone())
            .collect::<Vec<_>>()
    };

    json!({ "query": names("Query"), "subscription": names("Subscription") })
}

#[tokio::test(flavor = "multi_thread")]
async fn http_queries_to_a_gated_subgraph_need_its_group() {
    let stack = Stack::spawn().await;

    let denied = stack.post(Some(&stack.user()), "{ secret }").await;
    assert_yaml_snapshot!("http_denied", denied.snapshot());

    let allowed = stack.post(Some(&stack.tester()), "{ secret }").await;
    assert_yaml_snapshot!("http_allowed", allowed.snapshot());
}

#[tokio::test(flavor = "multi_thread")]
async fn ungated_fields_survive_a_denied_sibling() {
    let stack = Stack::spawn().await;

    let response = stack.post(Some(&stack.user()), "{ whoami secret }").await;
    assert_yaml_snapshot!(response.snapshot());
}

#[tokio::test(flavor = "multi_thread")]
async fn forged_tokens_do_not_pass_the_gate() {
    let stack = Stack::spawn().await;

    let response = stack.post(Some(&stack.forged()), "{ secret }").await;
    assert_yaml_snapshot!(response.snapshot());
}

#[tokio::test(flavor = "multi_thread")]
async fn subscriptions_to_a_gated_subgraph_need_its_group() {
    let stack = Stack::spawn().await;

    let mut socket = stack.connect(&stack.user()).await;
    socket
        .subscribe(
            "1",
            "subscription { secretTicks(count: 2, everyMs: 50) { n user } }",
        )
        .await;
    assert_yaml_snapshot!("subscription_denied", socket.messages("1").await);
    socket.close().await;
    assert_eq!(stack.live.count(), 0);

    let mut socket = stack.connect(&stack.tester()).await;
    socket
        .subscribe(
            "1",
            "subscription { secretTicks(count: 2, everyMs: 50) { n user } }",
        )
        .await;
    assert_yaml_snapshot!("subscription_allowed", socket.messages("1").await);
    socket.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn introspection_hides_gated_fields() {
    let stack = Stack::spawn().await;

    let user = stack.query(&stack.user(), SCHEMA).await;
    assert_yaml_snapshot!("introspection_user", root_fields(&user));

    let tester = stack.query(&stack.tester(), SCHEMA).await;
    assert_yaml_snapshot!("introspection_tester", root_fields(&tester));
}
