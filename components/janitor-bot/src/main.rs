use axum::{
    Router,
    body::Bytes,
    extract::{Json, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
};
use chrono_tz::Australia;
use janitor_bot::{argocd::ArgocdClient, clients::Clients, github::GitHubClient};
use janitor_bot::{command, event, github, rules};
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

    let forgejo_event = headers
        .get("X-Forgejo-Event")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let forgejo_event_type = headers
        .get("X-Forgejo-Event-Type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    tracing::info!(
        action = event.action,
        forgejo_event,
        forgejo_event_type,
        "received forgejo webhook event"
    );

    if forgejo_event == "issue_comment" && (forgejo_event_type != "pull_request_comment") {
        if let Some(cmd) = event.into_issue_comment_event()
            && cmd.author == FORGEJO_OWNER
            && let Some(parsed) = command::parse_issue_command(&cmd.comment_body)
        {
            tokio::spawn(async move {
                command::handle_issue_command(&state.clients, &cmd, parsed).await;
            });
        }
        return StatusCode::OK;
    }

    if forgejo_event == "issue_comment" && forgejo_event_type == "pull_request_comment" {
        if let Some(cmd) = event.into_comment_event()
            && cmd.author == FORGEJO_OWNER
            && let Some(parsed) = command::parse_pr_command(&cmd.body)
        {
            tokio::spawn(async move {
                command::handle_pr_command(&state.clients, &state.orchestrator, &cmd, parsed).await;
            });
        }
        return StatusCode::OK;
    }

    if forgejo_event != "pull_request" {
        return StatusCode::OK;
    }

    let Some(mut pr_event) = event.into_pr_event() else {
        return StatusCode::OK;
    };

    tokio::spawn(async move {
        state
            .orchestrator
            .evaluate_pr(&state.clients, &mut pr_event)
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

    let github_event = headers
        .get("X-GitHub-Event")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if github_event == "status" {
        let Some(cs_event) = github::parse_commit_status_event(&body) else {
            tracing::info!("received github status event but failed to parse");
            return StatusCode::OK;
        };
        tracing::info!(
            repository = cs_event.repository,
            context = cs_event.context,
            state = cs_event.state,
            "received github commit status event"
        );
        tokio::spawn(async move {
            state
                .orchestrator
                .evaluate_commit_status(&state.clients, &cs_event)
                .await;
        });
        return StatusCode::OK;
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
            .evaluate_workflow(&state.clients, &mut wf_event)
            .await;
    });

    StatusCode::OK
}

async fn evaluate_open_prs(state: &AppState) {
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
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let state = Arc::new(AppState {
        clients: Clients::new(
            ForgejoClient::from_env()?,
            GitHubClient::from_env()?,
            ArgocdClient::from_env()?,
            FeatureFlagClient::from_env().await,
        ),
        forgejo_webhook_secret: std::env::var("FORGEJO_INCOMING_WEBHOOK_AUTH")?,
        github_webhook_secret: std::env::var("GITHUB_WEBHOOK_SECRET")?,
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
        .route("/forgejo/webhook", post(handle_forgejo_webhook))
        .route("/github/webhook", post(handle_github_webhook))
        .with_state(state);

    let addr = "0.0.0.0:3000";
    tracing::info!("listening on {addr}");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
