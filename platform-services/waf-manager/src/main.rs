mod routes;

use futures::StreamExt;
use kube::{Api, Client, runtime::controller::Controller, runtime::watcher};
use routes::{AppState, Routes};
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;
use tower_http::services::ServeDir;
use waf_manager::controller::{
    Context, block_error_policy, policy_error_policy, reconcile_block, reconcile_policy,
};
use waf_manager::metrics::Metrics;
use waf_manager::{Allowlist, Loki, PolicyWriter, Result, WafBlock, WafPolicy};

const DEFAULT_LOKI: &str = "http://monitoring-loki.monitoring.svc.cluster.local:3100";
const DEFAULT_POLICY_NAMESPACE: &str = "envoy-gateway-system";

/// Anomaly-score aggregates and body-parse noise: consequences of other rules
/// rather than findings in their own right, so they are hidden from the ranking.
const DEFAULT_IGNORED_RULES: &[&str] = &["949110", "949111", "200002"];

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
    let window = env("DETECTION_WINDOW", "1h");

    let ignored_rules: BTreeSet<String> = std::env::var("IGNORED_RULE_IDS")
        .ok()
        .map(|raw| {
            raw.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_else(|| {
            DEFAULT_IGNORED_RULES
                .iter()
                .map(|s| s.to_string())
                .collect()
        });

    let resync = Duration::from_secs(
        env("RESYNC_SECONDS", "300")
            .parse()
            .expect("RESYNC_SECONDS must be a whole number of seconds"),
    );

    let client = Client::try_default().await?;
    let writer = PolicyWriter::new(client.clone(), &policy_namespace, &namespace).await?;
    let ctx = Arc::new(Context::new(
        client.clone(),
        namespace.clone(),
        Allowlist::new(),
        writer,
    ));

    // Converge once at startup so a restart repairs anything that drifted while
    // no controller was running.
    if let Err(e) = ctx.sync_all().await {
        tracing::warn!("initial sync failed: {e}");
    }

    let state = Arc::new(AppState {
        ctx: ctx.clone(),
        loki: Loki::new(loki_url),
        window,
        ignored_rules,
    });

    let app = Routes::router(state).nest_service("/static", ServeDir::new("static"));
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000")
        .await
        .expect("failed to bind :3000");

    tracing::info!("waf-manager listening on :3000, namespace {namespace}");

    // Deleting the last WafBlock reconciles a cached object and then has nothing
    // left to requeue, which can strand the SecurityPolicy still denying a CIDR
    // whose block is gone. A periodic rebuild converges regardless of events.
    let resync_ctx = ctx.clone();
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(resync);
        ticker.tick().await;

        loop {
            ticker.tick().await;
            if let Err(e) = resync_ctx.sync_all().await {
                tracing::warn!("periodic resync failed: {e}");
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
