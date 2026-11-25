use serde_json::json;

use crate::setup::Stack;

const INTROSPECTION: &str = "{ __schema { queryType { name } } }";

#[tokio::test(flavor = "multi_thread")]
async fn anonymous_callers_need_a_persisted_document() {
    let stack = Stack::spawn().await;

    let response = stack.post(None, "{ whoami }").await;
    assert_eq!(
        response.body["errors"][0]["extensions"]["code"],
        "PERSISTED_DOCUMENT_ID_REQUIRED"
    );

    let introspection = stack.post(None, INTROSPECTION).await;
    assert_eq!(
        introspection.body["errors"][0]["extensions"]["code"],
        "PERSISTED_DOCUMENT_ID_REQUIRED"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn anonymous_callers_cannot_introspect_through_a_document() {
    let stack = Stack::spawn().await;

    let response = stack.post_document(None, "introspect").await;
    assert_eq!(
        response.body["errors"][0]["extensions"]["code"],
        "INTROSPECTION_DISABLED"
    );
    assert!(response.body.get("data").is_none_or(|data| data.is_null()));
}

#[tokio::test(flavor = "multi_thread")]
async fn anonymous_errors_hide_the_subgraph() {
    let stack = Stack::spawn().await;

    let response = stack.post_document(None, "secret").await;
    for error in response.body["errors"].as_array().unwrap() {
        let extensions = error["extensions"].as_object().unwrap();
        assert_eq!(extensions.keys().collect::<Vec<_>>(), ["code"], "{error}");
        assert!(error.get("locations").is_none(), "{error}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn authenticated_callers_send_documents_or_queries() {
    let stack = Stack::spawn().await;

    let document = stack.post_document(Some(&stack.user()), "whoami").await;
    assert_eq!(document.body, json!({ "data": { "whoami": "tester" } }));

    let introspection = stack.post(Some(&stack.user()), INTROSPECTION).await;
    assert_eq!(
        introspection.body,
        json!({ "data": { "__schema": { "queryType": { "name": "Query" } } } })
    );

    let unknown = stack.post_document(Some(&stack.user()), "nope").await;
    assert_eq!(
        unknown.body["errors"][0]["extensions"]["code"],
        "PERSISTED_DOCUMENT_NOT_FOUND"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn callers_over_their_limit_are_rejected() {
    let stack = Stack::spawn_with_rate_limit(2).await;
    let token = stack.user();

    for _ in 0..2 {
        let response = stack.post(Some(&token), "{ whoami }").await;
        assert_eq!(response.status, 200, "{}", response.body);
    }

    let limited = stack.post(Some(&token), "{ whoami }").await;
    assert_eq!(limited.status, 429);
    assert_eq!(
        limited.body["errors"][0]["extensions"]["code"],
        "RATE_LIMITED"
    );
    assert!(
        limited.body["errors"][0]["extensions"]["retryAfterMs"]
            .as_u64()
            .unwrap()
            > 0
    );

    let anonymous = stack.post_document(None, "whoami").await;
    assert_ne!(anonymous.status, 429, "callers are limited separately");
}

#[tokio::test(flavor = "multi_thread")]
async fn anonymous_callers_are_limited_per_client_ip() {
    let stack = Stack::spawn_with_rate_limit(1).await;

    let first = stack.post_document_from("100.64.0.1", "whoami").await;
    assert_ne!(first.status, 429, "{}", first.body);

    let again = stack.post_document_from("100.64.0.1", "whoami").await;
    assert_eq!(again.status, 429);

    let other = stack.post_document_from("100.64.0.2", "whoami").await;
    assert_ne!(other.status, 429, "each client ip has its own bucket");
}

#[tokio::test(flavor = "multi_thread")]
async fn callers_over_their_cost_budget_are_rejected() {
    const THINGS: &str =
        r#"{ a: node(id: "open:thing:1") { id } b: node(id: "open:thing:2") { id } }"#;
    let stack = Stack::spawn_with_budget(1).await;
    let token = stack.user();

    let first = stack.post(Some(&token), THINGS).await;
    assert_eq!(first.status, 200, "{}", first.body);
    let cost = first.diagnostics.unwrap()["cost"]["actual"]
        .as_u64()
        .unwrap();
    assert!(
        cost > 1,
        "the query must cost more than the whole budget, got {cost}"
    );

    let limited = stack.post(Some(&token), THINGS).await;
    assert_eq!(limited.status, 429);
    let extensions = &limited.body["errors"][0]["extensions"];
    assert_eq!(extensions["code"], "RATE_LIMITED");
    assert_eq!(extensions["reason"], "cost");

    let ci = stack.forgejo("anurag/k8s", "refs/heads/main");
    let other = stack.post(Some(&ci), THINGS).await;
    assert_ne!(other.status, 429, "each caller has its own budget");
}

#[tokio::test(flavor = "multi_thread")]
async fn diagnostics_report_the_cost() {
    let stack = Stack::spawn().await;

    let response = stack.post(Some(&stack.user()), "{ whoami }").await;
    let cost = &response.diagnostics.unwrap()["cost"];
    assert_eq!(cost["max"], 10000);
    assert!(cost["estimated"].is_u64(), "{cost}");
}
