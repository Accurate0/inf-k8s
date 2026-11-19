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
async fn diagnostics_report_the_cost() {
    let stack = Stack::spawn().await;

    let response = stack.post(Some(&stack.user()), "{ whoami }").await;
    let cost = &response.diagnostics.unwrap()["cost"];
    assert_eq!(cost["max"], 10000);
    assert!(cost["estimated"].is_u64(), "{cost}");
}
