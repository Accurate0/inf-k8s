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
async fn external_tokens_reach_a_gated_subgraph_by_its_claim_patterns() {
    let stack = Stack::spawn().await;

    let main = stack.forgejo("anurag/k8s", "refs/heads/main");
    let allowed = stack.post(Some(&main), "{ secret }").await;
    assert_eq!(allowed.status, 200, "{}", allowed.body);
    assert_eq!(allowed.body["data"]["secret"], "hunter2");

    for token in [
        stack.forgejo("anurag/k8s", "refs/heads/feature"),
        stack.forgejo("someone/else", "refs/heads/main"),
    ] {
        let denied = stack.post(Some(&token), "{ secret }").await;
        assert_eq!(
            denied.body["data"]["secret"],
            Value::Null,
            "{}",
            denied.body
        );
        assert_eq!(denied.body["errors"][0]["extensions"]["code"], "FORBIDDEN");
    }
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

#[tokio::test(flavor = "multi_thread")]
async fn policy_fields_need_the_scope() {
    let stack = Stack::spawn().await;

    let user = stack
        .post(Some(&stack.user()), "{ whoami testerNote }")
        .await;
    assert!(user.body["data"]["whoami"].is_string(), "{}", user.body);
    assert!(user.body["data"]["testerNote"].is_null(), "{}", user.body);
    assert!(user.body["errors"].is_array(), "{}", user.body);

    let tester = stack.post(Some(&stack.tester()), "{ testerNote }").await;
    assert!(
        tester.body["data"]["testerNote"].is_null(),
        "groups are not scopes: {}",
        tester.body
    );

    let scoped = stack
        .post(Some(&stack.scoped("open:note:read")), "{ testerNote }")
        .await;
    assert_eq!(
        scoped.body["data"]["testerNote"], "for testers",
        "{}",
        scoped.body
    );
    assert!(scoped.body.get("errors").is_none(), "{}", scoped.body);

    let main = stack.forgejo("anurag/k8s", "refs/heads/main");
    let granted = stack.post(Some(&main), "{ testerNote }").await;
    assert_eq!(
        granted.body["data"]["testerNote"], "for testers",
        "{}",
        granted.body
    );

    let branch = stack.forgejo("anurag/k8s", "refs/heads/feature");
    let denied = stack.post(Some(&branch), "{ whoami testerNote }").await;
    assert!(
        denied.body["data"]["testerNote"].is_null(),
        "{}",
        denied.body
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn scoped_gates_admit_callers_holding_a_scope_without_the_group() {
    let stack = Stack::spawn().await;

    let plain = stack.post(Some(&stack.user()), "{ secret }").await;
    assert_eq!(
        plain.body["errors"][0]["extensions"]["code"], "FORBIDDEN",
        "{}",
        plain.body
    );

    let scoped = stack
        .post(Some(&stack.scoped("gated:secret:read")), "{ secret }")
        .await;
    assert_eq!(scoped.body["data"]["secret"], "hunter2", "{}", scoped.body);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_router_serves_its_passport_keys() {
    let stack = Stack::spawn().await;

    let (status, body) = stack.get_text(subgraph::passport::JWKS_PATH).await;
    assert_eq!(status, 200, "{body}");
    let jwks: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(jwks["keys"][0]["kid"], stack.passport_kid());
    assert_eq!(jwks["keys"][0]["alg"], "ES256");
    assert!(
        jwks["keys"][0].get("d").is_none(),
        "the private key must not be served"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn client_sent_passports_never_reach_a_subgraph() {
    let stack = Stack::spawn().await;
    let smuggled = stack.smuggled_passport("open", "intruder");

    let anonymous = stack
        .post_with_headers(
            None,
            json!({ "documentId": "whoami" }),
            &[(subgraph::passport::HEADER, &smuggled)],
        )
        .await;
    assert!(anonymous.body["data"].is_null(), "{}", anonymous.body);
    assert_eq!(
        anonymous.body["errors"][0]["extensions"]["code"], "UNAUTHENTICATED",
        "{}",
        anonymous.body
    );

    let user = stack
        .post_with_headers(
            Some(&stack.user()),
            json!({ "query": "{ whoami }" }),
            &[(subgraph::passport::HEADER, &smuggled)],
        )
        .await;
    assert_eq!(user.body["data"]["whoami"], "tester", "{}", user.body);

    let mut socket = stack
        .connect_with(json!({
            "Authorization": format!("Bearer {}", stack.user()),
            subgraph::passport::HEADER: smuggled,
        }))
        .await;
    socket
        .subscribe(
            "1",
            "subscription { ticks(count: 1, everyMs: 10) { n user } }",
        )
        .await;
    let messages = socket.messages("1").await;
    assert_eq!(
        messages[0]["payload"]["data"]["ticks"]["user"], "tester",
        "{messages:?}"
    );
    socket.close().await;
}
