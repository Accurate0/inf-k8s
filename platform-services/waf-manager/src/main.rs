mod routes;

use futures::StreamExt;
use kube::{Api, Client, runtime::controller::Controller, runtime::watcher};
use routes::{AppState, Routes};
use sqlx::postgres::PgPoolOptions;
use std::sync::Arc;
use std::time::Duration;
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::watch;
use tower_http::services::ServeDir;
use waf_manager::controller::{
    Context, SYNC_DEBOUNCE, block_error_policy, policy_error_policy, reconcile_block,
    reconcile_policy,
};
use waf_manager::metrics::Metrics;
use waf_manager::{
    Allowlist, Audit, Config, Enricher, Jwks, LeaderElector, Loki, ManualAllowlist, PolicyWriter,
    Result, Suppressions, WafBlock, WafPolicy, WorkflowEngine,
};

const DEFAULT_LOKI: &str = "http://monitoring-loki.monitoring.svc.cluster.local:3100";
const DEFAULT_POLICY_NAMESPACE: &str = "envoy-gateway-system";
const DB_MIN_CONNECTIONS: u32 = 0;
const DB_MAX_CONNECTIONS: u32 = 10;
const INSECURE_NO_AUTH: &str = "WAF_MANAGER_INSECURE_NO_AUTH";
const JWKS_REFRESH: Duration = Duration::from_secs(3600);

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().init();
    Metrics::init();

    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("failed to install rustls crypto provider");

    let namespace = env("POD_NAMESPACE", "waf-manager");
    let policy_namespace = env("POLICY_NAMESPACE", DEFAULT_POLICY_NAMESPACE);
    let loki_url = env("LOKI_URL", DEFAULT_LOKI);
    let identity = env("POD_NAME", "waf-manager");

    let config = Config::load();

    let resync = Duration::from_secs(
        env("RESYNC_SECONDS", "300")
            .parse()
            .expect("RESYNC_SECONDS must be a whole number of seconds"),
    );

    let fast_resync = Duration::from_secs(
        env("FAST_RESYNC_SECONDS", "20")
            .parse()
            .expect("FAST_RESYNC_SECONDS must be a whole number of seconds"),
    );

    let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let pool = PgPoolOptions::new()
        .min_connections(DB_MIN_CONNECTIONS)
        .max_connections(DB_MAX_CONNECTIONS)
        .connect(&database_url)
        .await?;

    sqlx::migrate!("./migrations").run(&pool).await?;

    let client = Client::try_default().await?;
    let writer = PolicyWriter::new(client.clone(), &policy_namespace, &namespace).await?;

    let allowlist = Allowlist::from_config(&config);

    let ctx = Arc::new(Context::new(
        client.clone(),
        namespace.clone(),
        allowlist.clone(),
        writer,
        pool.clone(),
        config.defaults.max_blocklist_cidrs,
    ));

    let loki = Arc::new(Loki::new(loki_url));

    let manual = ManualAllowlist::new(pool.clone(), allowlist.clone());

    if let Err(e) = manual.reload().await {
        tracing::error!("loading the manual allowlist failed, only config entries apply: {e}");
    }

    tokio::spawn(allowlist.clone().run(config.allowlist_refresh));

    let suppressions = Suppressions::new(
        pool.clone(),
        client.clone(),
        &namespace,
        config.manual_unblock_cooldown,
    );
    let engine = Arc::new(WorkflowEngine::new(
        config,
        loki.clone(),
        ctx.clone(),
        suppressions,
        pool.clone(),
    ));

    let elector = Arc::new(LeaderElector::new(
        client.clone(),
        &namespace,
        identity.clone(),
    ));
    let leadership = elector.subscribe();

    let jwks = match (Jwks::from_env(), std::env::var(INSECURE_NO_AUTH).is_ok()) {
        (Some(jwks), _) => {
            jwks.refresh().await.expect("initial jwks refresh failed");

            jwks.self_check()
                .await
                .expect("jwks self check failed, refusing to serve");

            tokio::spawn(jwks.clone().run(JWKS_REFRESH));
            Some(jwks)
        }
        (None, true) => {
            tracing::error!("{INSECURE_NO_AUTH} is set, serving every request unauthenticated");
            None
        }
        (None, false) => panic!("OIDC_ISSUER must be set, or {INSECURE_NO_AUTH} to opt out"),
    };

    let state = Arc::new(AppState {
        ctx: ctx.clone(),
        loki,
        engine: engine.clone(),
        leadership: leadership.clone(),
        jwks,
        audit: Audit::new(pool.clone()),
        manual,
        enricher: Enricher::new(),
    });

    let app = Routes::router(state).nest_service("/static", ServeDir::new("static"));
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000")
        .await
        .expect("failed to bind :3000");

    tracing::info!(
        workflows = engine.config().workflows.len(),
        "waf-manager listening on :3000, namespace {namespace}, identity {identity}"
    );

    tokio::spawn(supervise_leader_work(
        leadership,
        ctx.clone(),
        engine,
        client,
        namespace,
        resync,
        fast_resync,
    ));

    let running = elector.clone();
    tokio::spawn(async move { running.run().await });

    tokio::select! {
        result = axum::serve(listener, app) => {
            if let Err(e) = result {
                tracing::error!("http server stopped: {e}");
            }
        }
        _ = shutdown() => tracing::info!("shutting down"),
    }

    elector.release().await;

    Ok(())
}

async fn supervise_leader_work(
    mut leadership: watch::Receiver<bool>,
    ctx: Arc<Context>,
    engine: Arc<WorkflowEngine>,
    client: Client,
    namespace: String,
    resync: Duration,
    fast_resync: Duration,
) {
    let mut running: Option<tokio::task::JoinHandle<()>> = None;

    loop {
        let held = *leadership.borrow_and_update();

        match (held, running.take()) {
            (true, None) => {
                running = Some(tokio::spawn(lead(
                    ctx.clone(),
                    engine.clone(),
                    client.clone(),
                    namespace.clone(),
                    resync,
                    fast_resync,
                )));
            }
            (false, Some(handle)) => handle.abort(),
            (_, handle) => running = handle,
        }

        if leadership.changed().await.is_err() {
            return;
        }
    }
}

async fn lead(
    ctx: Arc<Context>,
    engine: Arc<WorkflowEngine>,
    client: Client,
    namespace: String,
    resync: Duration,
    fast_resync: Duration,
) {
    if let Err(e) = engine.import_suppressions().await {
        tracing::warn!("importing legacy suppressions failed: {e}");
    }

    ctx.request_sync();

    let resync_ctx = ctx.clone();
    let resync_engine = engine.clone();
    let ticker = async move {
        if let Err(e) = resync_engine.run_once().await {
            tracing::warn!("initial workflow run failed: {e}");
        }

        let mut ticker = tokio::time::interval(resync);
        ticker.tick().await;

        loop {
            ticker.tick().await;
            resync_ctx.request_sync();

            if let Err(e) = resync_engine.run_once().await {
                tracing::warn!("workflow run failed: {e}");
            }
        }
    };

    let fast_engine = engine.clone();
    let fast_ticker = async move {
        let mut ticker = tokio::time::interval(fast_resync);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        loop {
            ticker.tick().await;

            if let Err(e) = fast_engine.run_fast().await {
                tracing::warn!("fast workflow run failed: {e}");
            }
        }
    };

    let syncer = ctx.clone().run_syncer(SYNC_DEBOUNCE);

    let blocks = Controller::new(
        Api::<WafBlock>::namespaced(client.clone(), &namespace),
        watcher::Config::default(),
    )
    .run(reconcile_block, block_error_policy, ctx.clone())
    .for_each(|res| async move {
        if let Err(e) = res {
            tracing::warn!("block reconcile failed: {e}");
        }
    });

    let policies = Controller::new(
        Api::<WafPolicy>::namespaced(client, &namespace),
        watcher::Config::default(),
    )
    .run(reconcile_policy, policy_error_policy, ctx)
    .for_each(|res| async move {
        if let Err(e) = res {
            tracing::warn!("policy reconcile failed: {e}");
        }
    });

    tokio::select! {
        _ = syncer => tracing::error!("syncer stopped"),
        _ = ticker => tracing::error!("resync ticker stopped"),
        _ = fast_ticker => tracing::error!("fast ticker stopped"),
        _ = blocks => tracing::error!("block controller stopped"),
        _ = policies => tracing::error!("policy controller stopped"),
    }
}

async fn shutdown() {
    let mut term = signal(SignalKind::terminate()).expect("failed to listen for SIGTERM");

    tokio::select! {
        _ = term.recv() => {}
        _ = tokio::signal::ctrl_c() => {}
    }
}

fn env(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}
