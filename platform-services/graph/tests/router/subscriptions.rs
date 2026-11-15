use std::collections::HashMap;

use insta::assert_yaml_snapshot;
use serde_json::json;

use crate::setup::Stack;

#[tokio::test(flavor = "multi_thread")]
async fn stream_to_completion_and_close_cleanly() {
    let stack = Stack::spawn().await;
    let mut socket = stack.connect(&stack.user()).await;

    socket
        .subscribe(
            "1",
            "subscription { ticks(count: 3, everyMs: 50) { n user } }",
        )
        .await;
    assert_yaml_snapshot!(socket.messages("1").await);

    socket.close().await;
    stack.wait_live(0).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn outlive_the_request_timeout() {
    let stack = Stack::spawn().await;
    let mut socket = stack.connect(&stack.user()).await;

    socket
        .subscribe(
            "1",
            "subscription { ticks(count: 4, everyMs: 1000) { n user } }",
        )
        .await;
    assert_eq!(socket.ticks("1").await, [1, 2, 3, 4]);

    socket.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn multiplex_on_one_socket() {
    let stack = Stack::spawn().await;
    let mut socket = stack.connect(&stack.user()).await;

    for id in ["a", "b", "c"] {
        socket
            .subscribe(
                id,
                "subscription { ticks(count: 3, everyMs: 50) { n user } }",
            )
            .await;
    }

    let mut ticks: HashMap<String, Vec<i64>> = HashMap::new();
    let mut completed = 0;
    while completed < 3 {
        let message = socket.recv().await;
        let id = message["id"].as_str().unwrap().to_owned();

        match message["type"].as_str() {
            Some("next") => ticks
                .entry(id)
                .or_default()
                .push(message["payload"]["data"]["ticks"]["n"].as_i64().unwrap()),
            Some("complete") => completed += 1,
            _ => panic!("unexpected message {message}"),
        }
    }

    for id in ["a", "b", "c"] {
        assert_eq!(ticks[id], [1, 2, 3], "subscription {id}");
    }

    socket.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn completing_a_subscription_stops_it_at_the_subgraph() {
    let stack = Stack::spawn().await;
    let mut socket = stack.connect(&stack.user()).await;

    socket
        .subscribe(
            "1",
            "subscription { ticks(count: 1000, everyMs: 50) { n user } }",
        )
        .await;
    assert_eq!(socket.recv().await["type"], "next");
    stack.wait_live(1).await;

    socket.send(json!({ "id": "1", "type": "complete" })).await;
    stack.wait_live(0).await;

    socket.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn closing_the_socket_stops_them_at_the_subgraph() {
    let stack = Stack::spawn().await;
    let mut socket = stack.connect(&stack.user()).await;

    socket
        .subscribe(
            "1",
            "subscription { ticks(count: 1000, everyMs: 50) { n user } }",
        )
        .await;
    assert_eq!(socket.recv().await["type"], "next");
    stack.wait_live(1).await;

    socket.close().await;
    stack.wait_live(0).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn subgraph_stream_errors_reach_the_client() {
    let stack = Stack::spawn().await;
    let mut socket = stack.connect(&stack.user()).await;

    socket
        .subscribe("1", "subscription { failingTicks(after: 2) { n user } }")
        .await;
    assert_yaml_snapshot!(socket.messages("1").await);

    socket.close().await;
    stack.wait_live(0).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn invalid_subscriptions_are_rejected() {
    let stack = Stack::spawn().await;
    let mut socket = stack.connect(&stack.user()).await;

    socket
        .subscribe(
            "1",
            "subscription { ticks(count: 1, everyMs: 10) { nope } }",
        )
        .await;
    assert_yaml_snapshot!(socket.messages("1").await);

    socket.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn anonymous_subscriptions_are_rejected_by_the_subgraph() {
    let stack = Stack::spawn().await;
    let mut socket = stack.connect_anonymous().await;

    socket
        .subscribe(
            "1",
            "subscription { ticks(count: 1, everyMs: 10) { n user } }",
        )
        .await;
    assert_yaml_snapshot!(socket.messages("1").await);

    socket.close().await;
    assert_eq!(stack.live.count(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn queries_run_over_the_socket() {
    let stack = Stack::spawn().await;
    let mut socket = stack.connect(&stack.user()).await;

    socket.subscribe("1", "{ whoami }").await;
    assert_yaml_snapshot!(socket.messages("1").await);

    socket.close().await;
}
