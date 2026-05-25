mod routes;

use axum::{
    Router,
    http::StatusCode,
    routing::{get, post},
};
use chrono_tz::Australia;
use janitor_bot::{argocd::ArgocdClient, clients::Clients, github::GitHubClient, metrics};
use janitor_bot::{event, rules};
use janitor_bot::{feature_flag::FeatureFlagClient, forgejo::ForgejoClient};
use rules::RulesOrchestrator;
use std::sync::Arc;
use tokio_cron_scheduler::{JobBuilder, JobScheduler};

const FORGEJO_OWNER: &str = "anurag";
const WATCH_REPOS: &[&str] = &["k8s"];

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

    for repo in WATCH_REPOS {
        tracing::info!(owner = FORGEJO_OWNER, repo, "polling open PRs");

        let prs = match state
            .clients
            .forgejo
            .list_open_prs(FORGEJO_OWNER, repo)
            .await
        {
            Ok(prs) => prs,
            Err(e) => {
                tracing::error!(owner = FORGEJO_OWNER, repo, "failed to list open PRs: {e}");
                continue;
            }
        };

        total_prs += prs.len();

        for pr in &prs {
            let Some(mut pr_event) =
                event::PrEvent::from_api_pr(pr, FORGEJO_OWNER.to_owned(), repo.to_string())
            else {
                continue;
            };

            state
                .orchestrator
                .evaluate_pr(&state.clients, &mut pr_event)
                .await;
        }
    }

    metrics::record_cron_run(cron_start.elapsed(), total_prs);
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
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

    let scheduler = JobScheduler::new().await?;

    let poll_state = Arc::clone(&state);
    let job = JobBuilder::new()
        .with_timezone(Australia::Perth)
        .with_cron_job_type()
        .with_schedule("every 10 minutes")?
        .with_run_async(Box::new(move |uuid, mut _lock| {
            tracing::info!("running PR poll: {uuid}");
            let state = Arc::clone(&poll_state);
            Box::pin(async move {
                evaluate_open_prs(&state).await;
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
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .with_state(state);

    let addr = "0.0.0.0:3000";
    tracing::info!("listening on {addr}");
    let listener = tokio::net::TcpListener::bind(addr).await?;

    axum::serve(listener, app).await?;

    Ok(())
}
