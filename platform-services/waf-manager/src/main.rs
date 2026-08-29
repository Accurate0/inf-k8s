mod routes;

use futures::StreamExt;
use kube::{Api, Client, runtime::controller::Controller, runtime::watcher};
use routes::{AppState, Routes};
use std::sync::Arc;
use std::time::Duration;
use tower_http::services::ServeDir;
use waf_manager::controller::{
    Context, block_error_policy, policy_error_policy, reconcile_block, reconcile_policy,
};
use waf_manager::metrics::Metrics;
use waf_manager::{
    Allowlist, Config, Loki, PolicyWriter, Result, Suppressions, WafBlock, WafPolicy,
    WorkflowEngine,
};

const DEFAULT_LOKI: &str = "http://monitoring-loki.monitoring.svc.cluster.local:3100";
const DEFAULT_POLICY_NAMESPACE: &str = "envoy-gateway-system";

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().init();
    Metrics::init();

    // kube and reqwest each pull in a rustls provider, so neither is installed
    // automatically and the first TLS handshake panics. Pick one up front.
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("failed to install rustls crypto provider");

    let namespace = env("POD_NAMESPACE", "waf-manager");
    let policy_namespace = env("POLICY_NAMESPACE", DEFAULT_POLICY_NAMESPACE);
    let loki_url = env("LOKI_URL", DEFAULT_LOKI);

    // Windows, ignored rules and the automatic-block workflows all live in
    // config.yaml now, so they can change without a rebuild.
    let config = Config::load();

    let resync = Duration::from_secs(
        env("RESYNC_SECONDS", "300")
            .parse()
            .expect("RESYNC_SECONDS must be a whole number of seconds"),
    );

    let client = Client::try_default().await?;
    let writer = PolicyWriter::new(client.clone(), &policy_namespace, &namespace).await?;

    let allowlist = Allowlist::from_config(&config);

    // Load the feeds before anything can block, so the first workflow run already
    // knows GitHub's and Cloudflare's current ranges.
    allowlist.refresh_or_panic().await;

    let ctx = Arc::new(Context::new(
        client.clone(),
        namespace.clone(),
        allowlist.clone(),
        writer,
    ));

    // Converge once at startup so a restart repairs anything that drifted while
    // no controller was running.
    if let Err(e) = ctx.sync_all().await {
        tracing::warn!("initial sync failed: {e}");
    }

    let loki = Arc::new(Loki::new(loki_url));

    tokio::spawn(allowlist.run(config.allowlist_refresh));

    let suppressions =
        Suppressions::new(client.clone(), &namespace, config.manual_unblock_cooldown);
    let engine = Arc::new(WorkflowEngine::new(
        config,
        loki.clone(),
        ctx.clone(),
        suppressions,
    ));

    let state = Arc::new(AppState {
        ctx: ctx.clone(),
        loki,
        engine: engine.clone(),
    });

    let app = Routes::router(state).nest_service("/static", ServeDir::new("static"));
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000")
        .await
        .expect("failed to bind :3000");

    tracing::info!(
        workflows = engine.config().workflows.len(),
        "waf-manager listening on :3000, namespace {namespace}"
    );

    // Deleting the last WafBlock reconciles a cached object and then has nothing
    // left to requeue, which can strand the SecurityPolicy still denying a CIDR
    // whose block is gone. A periodic rebuild converges regardless of events.
    let resync_ctx = ctx.clone();
    let resync_engine = engine.clone();
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(resync);
        ticker.tick().await;

        loop {
            ticker.tick().await;
            if let Err(e) = resync_ctx.sync_all().await {
                tracing::warn!("periodic resync failed: {e}");
            }

            // After the sync, so the workflows see the blocklist the compositor
            // just applied. A Loki outage must not stall the reconcile loop.
            if let Err(e) = resync_engine.run_once().await {
                tracing::warn!("workflow run failed: {e}");
            }
        }
    });

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
        result = axum::serve(listener, app) => {
            if let Err(e) = result {
                tracing::error!("http server stopped: {e}");
            }
        }
        _ = blocks => tracing::error!("block controller stopped"),
        _ = policies => tracing::error!("policy controller stopped"),
    }

    Ok(())
}

fn env(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}
