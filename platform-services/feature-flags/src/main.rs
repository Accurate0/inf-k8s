use feature_flags::cache::CacheClient;
use feature_flags::config::Config;
use feature_flags::grpc::{AdminService, EvaluationService};
use feature_flags::pb::admin_server::AdminServer;
use feature_flags::pb::evaluation_server::EvaluationServer;
use feature_flags::snapshot::SnapshotManager;
use feature_flags::store::Store;
use feature_flags::tracing_setup;
use sqlx::postgres::PgPoolOptions;
use tonic::codec::CompressionEncoding;
use tonic::transport::Server;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _otel = tracing_setup::init();
    let config = Config::from_env();

    let pool = PgPoolOptions::new()
        .min_connections(0)
        .max_connections(10)
        .connect(&config.database_url)
        .await?;
    sqlx::migrate!("./migrations").run(&pool).await?;

    let store = Store::new(pool);
    let cache = CacheClient::from_env().await;
    let manager = SnapshotManager::bootstrap(store.clone(), cache).await?;

    tokio::spawn(manager.clone().listen(config.database_url.clone()));

    let (health_reporter, health_service) = tonic_health::server::health_reporter();
    health_reporter
        .set_serving::<EvaluationServer<EvaluationService>>()
        .await;
    health_reporter
        .set_serving::<AdminServer<AdminService>>()
        .await;

    let reflection = tonic_reflection::server::Builder::configure()
        .register_encoded_file_descriptor_set(feature_flags::pb::FILE_DESCRIPTOR_SET)
        .build_v1()?;

    let addr = config.grpc_addr.parse()?;
    tracing::info!("feature-flags gRPC listening on {addr}");

    Server::builder()
        .trace_fn(feature_flags::grpc::grpc_span)
        .add_service(health_service)
        .add_service(reflection)
        .add_service(
            EvaluationServer::new(EvaluationService::new(manager.clone()))
                .send_compressed(CompressionEncoding::Zstd)
                .send_compressed(CompressionEncoding::Gzip)
                .accept_compressed(CompressionEncoding::Zstd)
                .accept_compressed(CompressionEncoding::Gzip),
        )
        .add_service(AdminServer::new(AdminService::new(store, manager)))
        .serve(addr)
        .await?;

    Ok(())
}
