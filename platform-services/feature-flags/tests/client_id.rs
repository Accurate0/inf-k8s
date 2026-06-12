mod common;

use common::{connect_eval, eval_request, spawn_server};
use feature_flags::pb;
use sqlx::PgPool;

#[sqlx::test(migrations = "./migrations")]
async fn resolve_requires_client_id(pool: PgPool) {
    let (endpoint, server_handle) = spawn_server(pool).await;
    let mut client = connect_eval(&endpoint).await;

    let request = pb::ResolveRequest {
        flag_key: "anything".into(),
        context: None,
    };

    let err = client
        .resolve_boolean(request.clone())
        .await
        .expect_err("expected rejection");
    assert_eq!(err.code(), tonic::Code::Unauthenticated);

    let ok = client.resolve_boolean(eval_request(request)).await;
    assert!(ok.is_ok());

    server_handle.abort();
}
