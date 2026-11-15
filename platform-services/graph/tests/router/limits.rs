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
