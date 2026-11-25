use std::time::Duration;

use crate::setup::{BROKEN_SUPERGRAPH, Stack};

// only sdl that fails to parse is rejected; sdl that parses but is wrong replaces the good one, so
// composition in `ortzi app supergraph` is what guards the bucket
#[tokio::test(flavor = "multi_thread")]
async fn a_broken_supergraph_keeps_the_last_good_one_serving() {
    let stack = Stack::spawn().await;
    let token = stack.user();

    stack.replace_supergraph(BROKEN_SUPERGRAPH);
    tokio::time::sleep(Duration::from_secs(1)).await;

    let response = stack.post(Some(&token), "{ whoami }").await;
    assert_eq!(response.status, 200, "{}", response.body);
    assert!(
        response.body["data"]["whoami"].is_string(),
        "{}",
        response.body
    );
    assert!(stack.ready().await);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_router_that_never_loaded_a_supergraph_is_not_ready() {
    let stack = Stack::spawn_broken().await;

    tokio::time::sleep(Duration::from_secs(2)).await;
    assert!(!stack.ready().await);
}
