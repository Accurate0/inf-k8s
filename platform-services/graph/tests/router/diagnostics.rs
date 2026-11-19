use serde_json::{Value, json};

use crate::setup::Stack;

fn subgraphs(diagnostics: &Value) -> Vec<(String, u64)> {
    diagnostics["subgraphs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|fetch| {
            (
                fetch["name"].as_str().unwrap().to_owned(),
                fetch["errors"].as_u64().unwrap(),
            )
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn authenticated_responses_carry_diagnostics() {
    let stack = Stack::spawn().await;

    let response = stack.post(Some(&stack.user()), "{ whoami }").await;
    assert_eq!(response.body, json!({ "data": { "whoami": "tester" } }));

    let diagnostics = response.diagnostics.expect("diagnostics extension");
    assert_eq!(
        diagnostics["requestId"].as_str(),
        response.request_id.as_deref()
    );
    for field in ["durationMs", "planMs", "executeMs"] {
        assert!(diagnostics[field].is_f64(), "{field}: {diagnostics}");
    }
    assert!(diagnostics["subgraphs"][0]["durationMs"].is_f64());
    assert_eq!(subgraphs(&diagnostics), [("open".to_owned(), 0)]);
}

#[tokio::test(flavor = "multi_thread")]
async fn anonymous_responses_carry_no_diagnostics() {
    let stack = Stack::spawn().await;

    let response = stack.post_document(None, "whoami").await;
    assert!(response.diagnostics.is_none());
    assert!(response.request_id.is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn denied_and_failed_fetches_count_their_errors() {
    let stack = Stack::spawn().await;

    let response = stack
        .post(Some(&stack.user()), "{ whoami fail secret }")
        .await;
    let mut fetches = subgraphs(&response.diagnostics.unwrap());
    fetches.sort();
    assert_eq!(fetches, [("gated".to_owned(), 1), ("open".to_owned(), 1)]);
}

#[tokio::test(flavor = "multi_thread")]
async fn caller_request_ids_are_echoed() {
    let stack = Stack::spawn().await;

    let response = stack
        .post_with_request_id(Some(&stack.user()), "{ whoami }", Some("caller-123"))
        .await;
    assert_eq!(response.request_id.as_deref(), Some("caller-123"));
    assert_eq!(response.diagnostics.unwrap()["requestId"], "caller-123");
}
