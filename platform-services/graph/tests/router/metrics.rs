use crate::setup::Stack;

#[tokio::test(flavor = "multi_thread")]
async fn prometheus_exposes_request_subgraph_and_error_series() {
    let stack = Stack::spawn_with_rate_limit(2).await;
    let token = stack.user();

    let response = stack.post(Some(&token), "{ whoami }").await;
    assert_eq!(response.status, 200, "{}", response.body);

    let denied = stack.post(Some(&token), "{ whoami secret }").await;
    assert_eq!(denied.body["errors"][0]["extensions"]["code"], "FORBIDDEN");

    let limited = stack.post(Some(&token), "{ whoami }").await;
    assert_eq!(limited.status, 429);

    let (status, metrics) = stack.get_text("/metrics").await;
    assert_eq!(status, 200);

    assert!(metrics.contains(r#"subgraph_name="open""#));
    assert!(metrics.contains(r#"hive_router_graphql_errors_total{code="FORBIDDEN""#));
    assert!(
        metrics.lines().any(
            |line| line.starts_with("http_server_request_duration_seconds_count")
                && line.contains(r#"http_response_status_code="429""#)
        ),
        "rate limited requests are counted by status, not as graphql errors"
    );
}
