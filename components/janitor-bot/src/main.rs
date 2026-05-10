mod event;
mod forgejo;
mod rules;

use axum::{
    Router, extract::Json, extract::State, http::HeaderMap, http::StatusCode, routing::post,
};
use forgejo::ForgejoClient;
use std::sync::Arc;
use tokio_cron_scheduler::{JobBuilder, JobScheduler};

const FORGEJO_OWNER: &str = "anurag";
const WATCH_REPOS: &[&str] = &["k8s"];

struct AppState {
    client: ForgejoClient,
    webhook_secret: String,
}

async fn handle_webhook(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(event): Json<event::WebhookEvent>,
) -> StatusCode {
    let auth = headers
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if auth != state.webhook_secret {
        return StatusCode::UNAUTHORIZED;
    }

    tracing::info!(action = event.action, "received webhook event");

    let Some(mut pr_event) = event.into_pr_event() else {
        return StatusCode::OK;
    };

    tokio::spawn(async move {
        match state
            .client
            .get_pr_changed_files(&pr_event.owner, &pr_event.repo, pr_event.pr_number as i64)
            .await
        {
            Ok(files) => pr_event.changed_files = files,
            Err(e) => tracing::warn!(
                pr = pr_event.pr_number,
                "failed to fetch changed files: {e}"
            ),
        }

        let rules = rules::all_rules();
        rules::evaluate(&rules, &state.client, &pr_event).await;
    });

    StatusCode::OK
}

async fn evaluate_open_prs(client: &ForgejoClient) {
    let rules = rules::all_rules();

    for repo in WATCH_REPOS {
        tracing::info!(owner = FORGEJO_OWNER, repo, "polling open PRs");

        let prs = match client.list_open_prs(FORGEJO_OWNER, repo).await {
            Ok(prs) => prs,
            Err(e) => {
                tracing::error!(owner = FORGEJO_OWNER, repo, "failed to list open PRs: {e}");
                continue;
            }
        };

        for pr in &prs {
            let Some(mut pr_event) =
                event::PrEvent::from_api_pr(pr, FORGEJO_OWNER.to_owned(), repo.to_string())
            else {
                continue;
            };

            match client
                .get_pr_changed_files(FORGEJO_OWNER, repo, pr_event.pr_number as i64)
                .await
            {
                Ok(files) => pr_event.changed_files = files,
                Err(e) => {
                    tracing::warn!(
                        pr = pr_event.pr_number,
                        "failed to fetch changed files: {e}"
                    );
                    continue;
                }
            }

            rules::evaluate(&rules, client, &pr_event).await;
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let state = Arc::new(AppState {
        client: ForgejoClient::from_env()?,
        webhook_secret: std::env::var("FORGEJO_INCOMING_WEBHOOK_AUTH")?,
    });

    let scheduler = JobScheduler::new().await?;

    let poll_state = Arc::clone(&state);
    let job = JobBuilder::new()
        .with_timezone(chrono_tz::Australia::Perth)
        .with_cron_job_type()
        .with_schedule("every 10 minutes")?
        .with_run_async(Box::new(move |uuid, mut _lock| {
            tracing::info!("running PR poll: {uuid}");
            let state = Arc::clone(&poll_state);
            Box::pin(async move {
                evaluate_open_prs(&state.client).await;
            })
        }))
        .build()?;

    scheduler.add(job).await?;
    scheduler.start().await?;

    // Run once on startup
    let startup_state = Arc::clone(&state);
    tokio::spawn(async move {
        evaluate_open_prs(&startup_state.client).await;
    });

    let app = Router::new()
        .route("/webhook", post(handle_webhook))
        .with_state(state);

    let addr = "0.0.0.0:3000";
    tracing::info!("listening on {addr}");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
