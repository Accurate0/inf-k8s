#![allow(dead_code)]

use feature_flags::grpc::{AdminService, EvaluationService};
use feature_flags::pb::admin_client::AdminClient;
use feature_flags::pb::admin_server::AdminServer;
use feature_flags::pb::evaluation_client::EvaluationClient;
use feature_flags::pb::evaluation_server::EvaluationServer;
use feature_flags::snapshot::SnapshotManager;
use feature_flags::store::Store;
use sqlx::PgPool;
use tokio::task::JoinHandle;
use tonic::transport::Channel;

pub async fn spawn_server(pool: PgPool) -> (String, JoinHandle<()>) {
    let store = Store::new(pool);
    let manager = SnapshotManager::bootstrap(store.clone(), None)
        .await
        .unwrap();
    let admin = AdminService::new(store, manager.clone());
    let evaluation = EvaluationService::new(manager);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);
    let handle = tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(EvaluationServer::new(evaluation))
            .add_service(AdminServer::new(admin))
            .serve_with_incoming(incoming)
            .await
            .unwrap();
    });
    (format!("http://{addr}"), handle)
}

pub async fn connect_admin(endpoint: &str) -> AdminClient<Channel> {
    AdminClient::connect(endpoint.to_string()).await.unwrap()
}

pub async fn connect_eval(endpoint: &str) -> EvaluationClient<Channel> {
    EvaluationClient::connect(endpoint.to_string())
        .await
        .unwrap()
}

pub fn eval_request<T>(msg: T) -> tonic::Request<T> {
    let mut request = tonic::Request::new(msg);
    request
        .metadata_mut()
        .insert("client-id", "integration-test".parse().unwrap());
    request
}
