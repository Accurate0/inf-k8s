use tokio_tungstenite::tungstenite;

use crate::setup::Stack;

#[tokio::test(flavor = "multi_thread")]
async fn closed_sockets_free_their_long_lived_client_slot() {
    let stack = Stack::spawn().await;

    for _ in 0..4 {
        let mut socket = stack.connect(&stack.user()).await;
        socket
            .subscribe(
                "1",
                "subscription { ticks(count: 1, everyMs: 10) { n user } }",
            )
            .await;
        assert_eq!(socket.ticks("1").await, [1]);
        socket.close().await;
    }

    let held = [
        stack.connect(&stack.user()).await,
        stack.connect(&stack.user()).await,
    ];
    let rejected = stack
        .upgrade()
        .await
        .err()
        .expect("third socket was accepted");
    let tungstenite::Error::Http(response) = rejected else {
        panic!("unexpected error {rejected}");
    };
    assert_eq!(response.status(), 503);

    for socket in held {
        socket.close().await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn slow_subgraphs_time_out_instead_of_hanging() {
    let stack = Stack::spawn().await;
    let token = stack.user();

    let quick = stack.post(Some(&token), "{ slow(ms: 10) }").await;
    assert_eq!(quick.body["data"]["slow"], true, "{}", quick.body);

    let started = std::time::Instant::now();
    let slow = stack.post(Some(&token), "{ whoami slow(ms: 5000) }").await;
    assert!(
        started.elapsed() < std::time::Duration::from_millis(1900),
        "the 1s subgraph timeout fires before the router's own 2s one"
    );
    assert!(slow.body["errors"].is_array(), "{}", slow.body);
}
