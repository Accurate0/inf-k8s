mod event;
mod forgejo;
mod github;
mod rules;
mod schema;

use axum::{
    Router,
    body::Bytes,
    extract::{Json, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
};
use forgejo::ForgejoClient;
use rules::RulesOrchestrator;
use std::sync::Arc;
use tokio_cron_scheduler::{JobBuilder, JobScheduler};

const FORGEJO_OWNER: &str = "anurag";
const WATCH_REPOS: &[&str] = &["k8s"];

struct AppState {
    client: ForgejoClient,
    github_client: github::GitHubClient,
    forgejo_webhook_secret: String,
    github_webhook_secret: String,
    orchestrator: RulesOrchestrator,
}

async fn handle_forgejo_webhook(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(event): Json<event::WebhookEvent>,
) -> StatusCode {
    let auth = headers
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if auth != state.forgejo_webhook_secret {
        return StatusCode::UNAUTHORIZED;
    }

    tracing::info!(action = event.action, "received forgejo webhook event");

    let Some(mut pr_event) = event.into_pr_event() else {
        return StatusCode::OK;
    };

    tokio::spawn(async move {
        state
            .orchestrator
            .evaluate_pr(&state.client, &mut pr_event)
            .await;
    });

    StatusCode::OK
}

async fn handle_github_webhook(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    let signature = headers
        .get("X-Hub-Signature-256")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if !github::verify_signature(&state.github_webhook_secret, signature, &body) {
        return StatusCode::UNAUTHORIZED;
    }

    let Some(mut wf_event) = github::parse_workflow_event(&body) else {
        tracing::info!("received github webhook (not a workflow_run event, ignoring)");
        return StatusCode::OK;
    };

    tracing::info!(
        workflow = wf_event.workflow_name,
        conclusion = wf_event.conclusion,
        "received github workflow event"
    );

    tokio::spawn(async move {
        state
            .orchestrator
            .evaluate_workflow(&state.client, &state.github_client, &mut wf_event)
            .await;
    });

    StatusCode::OK
}

async fn evaluate_open_prs(state: &AppState) {
    let client = &state.client;

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

            state.orchestrator.evaluate_pr(client, &mut pr_event).await;
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let state = Arc::new(AppState {
        client: ForgejoClient::from_env()?,
        github_client: github::GitHubClient::from_env()?,
        forgejo_webhook_secret: std::env::var("FORGEJO_INCOMING_WEBHOOK_AUTH")?,
        github_webhook_secret: std::env::var("GITHUB_WEBHOOK_SECRET")?,
        orchestrator: RulesOrchestrator::new(),
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
                evaluate_open_prs(&state).await;
            })
        }))
        .build()?;

    scheduler.add(job).await?;
    scheduler.start().await?;

    let app = Router::new()
        .route("/health", get(|| async { StatusCode::OK }))
        .route("/forgejo/webhook", post(handle_forgejo_webhook))
        .route("/github/webhook", post(handle_github_webhook))
        .with_state(state);

    let addr = "0.0.0.0:3000";
    tracing::info!("listening on {addr}");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
