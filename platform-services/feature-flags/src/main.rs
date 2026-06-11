use feature_flags::cache::CacheClient;
use feature_flags::config::Config;
use feature_flags::grpc::{AdminService, EvaluationService};
use feature_flags::pb::admin_server::AdminServer;
use feature_flags::pb::evaluation_server::EvaluationServer;
use feature_flags::snapshot::SnapshotManager;
use feature_flags::store::Store;
use feature_flags::{metrics, tracing_setup};
use sqlx::postgres::PgPoolOptions;
use tonic::transport::Server;

// priority order
// TODO: replace local crate path with github for downstream consumers (also means crate changes
// dont redeploy multiple things)
// TODO: frontend interface with oidc (envoy oidc)
// TODO: frontend should have debugging tab like snapshot stream, and test evaluation (call server
// side eval)
// TODO: boolean flags should default to having variants true and false
// TODO: allow feature flag provisioning from flags.yaml in repo
// TODO: feature flags enabled by default
// TODO: handle snapshot version number getting too big
// TODO: fix prometheus metrics
// TODO: better logging of stream connection start and grpc requests
// TODO: make sure multiple replicas works, run as daemonset with local routing preferred
// TODO: integration testing
// TODO: use compression for grpc snapshots in particular

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _otel = tracing_setup::init();
    let config = Config::from_env();

    metrics::init(config.metrics_addr.parse()?)?;

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
        .add_service(health_service)
        .add_service(reflection)
        .add_service(EvaluationServer::new(EvaluationService::new(
            manager.clone(),
        )))
        .add_service(AdminServer::new(AdminService::new(store, manager)))
        .serve(addr)
        .await?;

    Ok(())
}
