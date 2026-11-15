use insta::assert_yaml_snapshot;

use crate::setup::Stack;

#[tokio::test(flavor = "multi_thread")]
async fn queries_reach_the_subgraph_with_the_callers_token() {
    let stack = Stack::spawn().await;

    let response = stack.post(Some(&stack.user()), "{ whoami }").await;
    assert_yaml_snapshot!(response.snapshot());
}

#[tokio::test(flavor = "multi_thread")]
async fn queries_without_a_token_are_rejected_by_the_subgraph() {
    let stack = Stack::spawn().await;

    let response = stack.post(None, "{ whoami }").await;
    assert_yaml_snapshot!(response.snapshot());
}

#[tokio::test(flavor = "multi_thread")]
async fn queries_with_a_forged_token_are_rejected_by_the_subgraph() {
    let stack = Stack::spawn().await;

    let response = stack.post(Some(&stack.forged()), "{ whoami }").await;
    assert_yaml_snapshot!(response.snapshot());
}

#[tokio::test(flavor = "multi_thread")]
async fn invalid_queries_are_rejected_before_planning() {
    let stack = Stack::spawn().await;

    let response = stack.post(Some(&stack.user()), "{ whoami nope }").await;
    assert_yaml_snapshot!(response.snapshot());
}

#[tokio::test(flavor = "multi_thread")]
async fn subgraph_errors_keep_their_path() {
    let stack = Stack::spawn().await;

    let response = stack.post(Some(&stack.user()), "{ whoami fail }").await;
    assert_yaml_snapshot!(response.snapshot());
}

#[tokio::test(flavor = "multi_thread")]
async fn subscriptions_stream_over_server_sent_events() {
    let stack = Stack::spawn().await;

    let events = stack
        .sse(
            &stack.user(),
            "subscription { ticks(count: 3, everyMs: 10) { n user } }",
        )
        .await;
    assert_yaml_snapshot!(events.lines().collect::<Vec<_>>());
    stack.wait_live(0).await;
}
