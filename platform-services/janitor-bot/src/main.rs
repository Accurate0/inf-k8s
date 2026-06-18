mod routes;

use axum::{
    Router,
    http::StatusCode,
    routing::{get, post},
};
use axum_tracing_opentelemetry::middleware::{OtelAxumLayer, OtelInResponseLayer};
use chrono_tz::Australia;
use janitor_bot::{argocd::ArgocdClient, clients::Clients, github::GitHubClient, metrics};
use janitor_bot::{event, rules, tracing_setup};
use janitor_bot::{feature_flag::FeatureFlagClient, forgejo::ForgejoClient};
use notify::{RecursiveMode, Watcher};
use rules::RulesOrchestrator;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio_cron_scheduler::{JobBuilder, JobScheduler};
use tracing::Instrument;

const FORGEJO_OWNER: &str = "anurag";

/// Watch the ConfigMap directory and hot-reload the orchestrator on change.
/// We watch the directory, not the file: a ConfigMap volume swaps its `..data`
/// symlink atomically on update. No-op when `RULES_CONFIGMAP_PATH` is unset.
fn spawn_rules_watcher(state: Arc<AppState>) -> anyhow::Result<()> {
    let Ok(path) = std::env::var(rules::RULES_CONFIGMAP_PATH_ENV) else {
        return Ok(());
    };
    let watch_dir = std::path::Path::new(&path)
        .parent()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| std::path::PathBuf::from("."));

    std::thread::Builder::new()
        .name("rules-watcher".into())
        .spawn(move || {
            let (tx, rx) = std::sync::mpsc::channel();
            let mut watcher = match notify::recommended_watcher(tx) {
                Ok(w) => w,
                Err(e) => {
                    tracing::error!("failed to create rules watcher: {e}");
                    return;
                }
            };
            if let Err(e) = watcher.watch(&watch_dir, RecursiveMode::NonRecursive) {
                tracing::error!(dir = %watch_dir.display(), "failed to watch rules dir: {e}");
                return;
            }
            tracing::info!(dir = %watch_dir.display(), "watching rules directory for changes");

            for res in rx {
                match res {
                    Ok(event)
                        if event.kind.is_modify()
                            || event.kind.is_create()
                            || event.kind.is_remove() =>
                    {
                        tracing::info!(?event.kind, "rules directory changed, reloading");
                        state.orchestrator.reload();
                    }
                    Ok(_) => {}
                    Err(e) => tracing::error!("rules watch error: {e}"),
                }
            }
        })?;

    Ok(())
}

struct AppState {
    clients: Clients,
    forgejo_webhook_secret: String,
    github_webhook_secret: String,
    argocd_webhook_secret: String,
    orchestrator: RulesOrchestrator,
}

async fn evaluate_open_prs(state: &AppState) {
    let cron_start = std::time::Instant::now();
    let mut total_prs = 0usize;

    for (owner, repo) in state.orchestrator.watch_repos() {
        tracing::info!(owner, repo, "polling open PRs");

        let prs = match state.clients.forgejo.list_open_prs(&owner, &repo).await {
            Ok(prs) => prs,
            Err(e) => {
                tracing::error!(owner, repo, "failed to list open PRs: {e}");
                continue;
            }
        };

        total_prs += prs.len();

        for pr in &prs {
            let Some(mut pr_event) = event::PrEvent::from_api_pr(pr, owner.clone(), repo.clone())
            else {
                continue;
            };

            let span = tracing::info_span!(
                "cron.evaluate",
                otel.name = "cron.evaluate: forgejo_pr",
                owner = pr_event.owner,
                repo = pr_event.repo,
                pr_number = pr_event.pr_number,
            );
            state
                .orchestrator
                .evaluate_pr(&state.clients, &mut pr_event)
                .instrument(span)
                .await;
        }
    }

    metrics::record_cron_run(cron_start.elapsed(), total_prs);
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let tracer_provider = tracing_setup::init();
    metrics::init();

    let state = Arc::new(AppState {
        clients: Clients::new(
            ForgejoClient::from_env()?,
            GitHubClient::from_env()?,
            ArgocdClient::from_env()?,
            FeatureFlagClient::from_env().await,
        ),
        forgejo_webhook_secret: std::env::var("FORGEJO_INCOMING_WEBHOOK_AUTH")?,
        github_webhook_secret: std::env::var("GITHUB_WEBHOOK_SECRET")?,
        argocd_webhook_secret: std::env::var("ARGOCD_WEBHOOK_SECRET").unwrap_or_default(),
        orchestrator: RulesOrchestrator::new(),
    });

    spawn_rules_watcher(Arc::clone(&state))?;

    let scheduler = JobScheduler::new().await?;

    let poll_state = Arc::clone(&state);
    let job = JobBuilder::new()
        .with_timezone(Australia::Perth)
        .with_cron_job_type()
        .with_schedule("every 10 minutes")?
        .with_run_async(Box::new(move |uuid, mut _lock| {
            let state = Arc::clone(&poll_state);
            Box::pin(async move {
                let span = tracing::info_span!(parent: None, "cron.evaluate_open_prs", job = %uuid);
                async move {
                    tracing::info!("running PR poll: {uuid}");
                    evaluate_open_prs(&state).await;
                }
                .instrument(span)
                .await;
            })
        }))
        .build()?;

    scheduler.add(job).await?;
    scheduler.start().await?;

    let app = Router::new()
        .route("/health", get(|| async { StatusCode::OK }))
        .route("/forgejo/webhook", post(routes::handle_forgejo_webhook))
        .route("/github/webhook", post(routes::handle_github_webhook))
        .route("/argocd/webhook", post(routes::handle_argocd_webhook))
        .route("/admin/cron", post(routes::handle_admin_cron))
        .route(
            "/admin/merge-queued",
            post(routes::handle_admin_merge_queued),
        )
        .route("/admin/command", post(routes::handle_admin_command))
        .route(
            "/admin/argocd-resync/{app}",
            post(routes::handle_admin_argocd_resync),
        )
        .route(
            "/admin/evaluate/{owner}/{repo}/{pr_number}",
            post(routes::handle_admin_evaluate_pr),
        )
        .route(
            "/admin/dry-run/{owner}/{repo}/{pr_number}",
            post(routes::handle_admin_dry_run),
        )
        .route("/admin/metrics", get(routes::handle_admin_metrics))
        .route("/admin/logs", get(routes::handle_admin_logs))
        .route("/admin/rules", get(routes::handle_admin_rules))
        .route("/admin/health/deep", get(routes::handle_admin_health_deep))
        .layer(OtelInResponseLayer)
        .layer(
            OtelAxumLayer::default().filter(|path| !matches!(path, "/health" | "/admin/metrics")),
        )
        .with_state(state);

    let port = std::env::var("PORT").unwrap_or_else(|_| "3000".to_string());
    let addr = format!("0.0.0.0:{port}");
    tracing::info!("listening on {addr}");
    let listener = TcpListener::bind(&addr).await?;

    axum::serve(listener, app).await?;

    if let Some(provider) = tracer_provider {
        let _ = provider.shutdown();
    }

    Ok(())
}
