use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use cli_auth::{App, OAuthClient, TokenSource};
use reqwest::Method;
use serde_json::Value;
use std::path::PathBuf;

const APP: App = App {
    name: "janitor",
    scopes: "openid email profile groups",
    login_hint: "run `k8sctl janitor login` or set JANITOR_TOKEN",
};

#[derive(Parser)]
#[command(name = "janitor", about = "CLI for the janitor-bot admin API")]
pub struct Cli {
    /// Base URL of janitor-bot (e.g. http://localhost:3000)
    #[arg(
        long,
        global = true,
        env = "JANITOR_URL",
        default_value = "https://janitor-bot.inf-k8s.net"
    )]
    url: String,

    #[arg(
        long,
        global = true,
        env = "JANITOR_OIDC_ISSUER",
        default_value = "https://idm.anurag.sh/oauth2/openid/janitor-bot",
        help = "OIDC issuer used to log in"
    )]
    issuer: String,

    #[arg(
        long,
        global = true,
        env = "JANITOR_OIDC_CLIENT_ID",
        default_value = "janitor-bot",
        help = "OAuth client id"
    )]
    client_id: String,

    #[arg(
        long,
        global = true,
        env = "JANITOR_TOKEN",
        hide_env_values = true,
        help = "Use this bearer token instead of the stored login"
    )]
    token: Option<String>,

    #[command(subcommand)]
    cmd: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Log in through the browser and store the session
    Login,
    /// Forget the stored session
    Logout,
    /// Show the logged in user
    Whoami,

    /// Trigger the open-PR cron evaluation now
    Cron,

    /// Re-evaluate a specific PR
    Evaluate {
        owner: String,
        repo: String,
        pr_number: i64,
    },

    /// Explain what rules would match for a PR without executing actions
    DryRun {
        owner: String,
        repo: String,
        pr_number: i64,
    },

    /// Merge all PRs currently labeled janitor/queued
    MergeQueued,

    /// Dispatch a @janitor command against a PR or issue
    #[command(name = "command")]
    Dispatch {
        #[arg(value_enum)]
        kind: CommandKind,
        owner: String,
        repo: String,
        number: i64,
        /// Command body, e.g. "@janitor merge squash"
        body: String,
    },

    /// Trigger an ArgoCD application resync
    ArgocdResync {
        /// ArgoCD application name
        app: String,
    },

    /// Dump prometheus metrics
    Metrics,

    /// Show the orchestrator evaluation log
    Logs,

    /// Show the compiled rule summary
    Rules,

    /// Deep health check (forgejo + github + argocd)
    Health,

    /// Replay a saved webhook payload against a (local) janitor-bot through the
    /// real, signature-verified webhook endpoint. The event header(s) are
    /// inferred from the payload. Reads the same secret env vars the bot does
    /// (FORGEJO_INCOMING_WEBHOOK_AUTH / GITHUB_WEBHOOK_SECRET /
    /// ARGOCD_WEBHOOK_SECRET). Returns 200 immediately; output is in the bot's logs.
    Replay {
        /// Which webhook endpoint to hit
        #[arg(value_enum)]
        source: Source,
        /// Path to a file containing the raw JSON webhook body
        file: PathBuf,
        /// Override the inferred event header (X-GitHub-Event / X-Forgejo-Event)
        #[arg(long)]
        event: Option<String>,
        /// Override the inferred X-Forgejo-Event-Type (forgejo only)
        #[arg(long)]
        event_type: Option<String>,
    },
}

#[derive(ValueEnum, Clone, Copy, Debug)]
enum CommandKind {
    Pr,
    Issue,
}

#[derive(ValueEnum, Clone, Copy, Debug)]
enum Source {
    Forgejo,
    Github,
    Argocd,
}

/// Runs janitor, mounted as `k8sctl janitor`.
pub async fn run(cli: Cli) -> Result<()> {
    let client = reqwest::Client::new();
    let base = cli.url.trim_end_matches('/');
    let oauth = OAuthClient::new(APP, &cli.issuer, &cli.client_id);

    match &cli.cmd {
        Command::Login => return cli_auth::commands::login(&oauth).await,
        Command::Logout => return cli_auth::commands::logout(&oauth),
        Command::Whoami => return cli_auth::commands::whoami(&oauth, base),
        _ => {}
    }

    if let Command::Replay {
        source,
        file,
        event,
        event_type,
    } = &cli.cmd
    {
        return run_replay(
            &client,
            base,
            *source,
            file,
            event.as_deref(),
            event_type.as_deref(),
        )
        .await;
    }

    let (method, path, body): (Method, String, Option<Value>) = match cli.cmd {
        Command::Cron => (Method::POST, "/admin/cron".into(), None),
        Command::Evaluate {
            owner,
            repo,
            pr_number,
        } => (
            Method::POST,
            format!("/admin/evaluate/{owner}/{repo}/{pr_number}"),
            None,
        ),
        Command::DryRun {
            owner,
            repo,
            pr_number,
        } => (
            Method::POST,
            format!("/admin/dry-run/{owner}/{repo}/{pr_number}"),
            None,
        ),
        Command::MergeQueued => (Method::POST, "/admin/merge-queued".into(), None),
        Command::Dispatch {
            kind,
            owner,
            repo,
            number,
            body,
        } => {
            let kind = match kind {
                CommandKind::Pr => "pr",
                CommandKind::Issue => "issue",
            };
            (
                Method::POST,
                "/admin/command".into(),
                Some(serde_json::json!({
                    "owner": owner,
                    "repo": repo,
                    "number": number,
                    "kind": kind,
                    "body": body,
                })),
            )
        }
        Command::ArgocdResync { app } => {
            (Method::POST, format!("/admin/argocd-resync/{app}"), None)
        }
        Command::Metrics => (Method::GET, "/admin/metrics".into(), None),
        Command::Logs => (Method::GET, "/admin/logs".into(), None),
        Command::Rules => (Method::GET, "/admin/rules".into(), None),
        Command::Health => (Method::GET, "/admin/health/deep".into(), None),
        Command::Replay { .. } => unreachable!("replay is handled before this match"),
        Command::Login | Command::Logout | Command::Whoami => {
            unreachable!("session commands are handled before this match")
        }
    };

    let url = format!("{base}{path}");
    let mut req = client.request(method, &url);
    // Prometheus scrapes /admin/metrics unauthenticated; everything else needs a bearer.
    if path != "/admin/metrics" {
        let token = match cli.token.filter(|token| !token.is_empty()) {
            Some(token) => token,
            None => TokenSource::acquire(oauth).await?.access_token(),
        };
        req = req.bearer_auth(token);
    }
    if let Some(b) = body {
        req = req.json(&b);
    }

    let resp = req
        .send()
        .await
        .with_context(|| format!("request to {url}"))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();

    if let Ok(v) = serde_json::from_str::<Value>(&text) {
        println!("{}", serde_json::to_string_pretty(&v)?);
    } else {
        print!("{text}");
    }

    if !status.is_success() {
        bail!("request failed: {status}");
    }
    Ok(())
}

fn require_env(var: &str) -> Result<String> {
    std::env::var(var).with_context(|| format!("{var} must be set to sign/authorize the replay"))
}

async fn run_replay(
    client: &reqwest::Client,
    base: &str,
    source: Source,
    file: &std::path::Path,
    event: Option<&str>,
    event_type: Option<&str>,
) -> Result<()> {
    let body = std::fs::read(file).with_context(|| format!("reading {}", file.display()))?;

    let (path, headers): (&str, Vec<(String, String)>) = match source {
        Source::Github => {
            let event = match event {
                Some(e) => e.to_string(),
                None => janitor_webhook::github::infer_event(&body)
                    .context(
                        "could not infer X-GitHub-Event from payload; pass --event \
                         (status|check_run|push|workflow_run)",
                    )?
                    .to_string(),
            };
            let secret = require_env("GITHUB_WEBHOOK_SECRET")?;
            let signature = janitor_webhook::github::sign_payload(&secret, &body);
            (
                "/github/webhook",
                vec![
                    ("X-GitHub-Event".into(), event),
                    ("X-Hub-Signature-256".into(), signature),
                ],
            )
        }
        Source::Forgejo => {
            let (event, inferred_type) = match event {
                Some(e) => (e.to_string(), event_type),
                None => {
                    let (e, t) = janitor_webhook::forgejo::infer_event(&body).context(
                        "could not infer X-Forgejo-Event from payload; pass --event \
                         (pull_request|issue_comment|push)",
                    )?;
                    (e.to_string(), event_type.or(t))
                }
            };
            let auth = require_env("FORGEJO_INCOMING_WEBHOOK_AUTH")?;
            let mut headers = vec![
                ("Authorization".into(), auth),
                ("X-Forgejo-Event".into(), event),
            ];
            if let Some(t) = inferred_type {
                headers.push(("X-Forgejo-Event-Type".into(), t.to_string()));
            }
            ("/forgejo/webhook", headers)
        }
        Source::Argocd => {
            let auth = require_env("ARGOCD_WEBHOOK_SECRET")?;
            ("/argocd/webhook", vec![("Authorization".into(), auth)])
        }
    };

    let url = format!("{base}{path}");
    let mut req = client
        .post(&url)
        .header("Content-Type", "application/json")
        .body(body);
    for (k, v) in &headers {
        req = req.header(k, v);
    }

    let event_desc = headers
        .iter()
        .filter(|(k, _)| k.starts_with("X-"))
        .map(|(k, v)| format!("{k}: {v}"))
        .collect::<Vec<_>>()
        .join(", ");
    eprintln!("replaying to {url} [{event_desc}]");

    let resp = req
        .send()
        .await
        .with_context(|| format!("request to {url}"))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !text.is_empty() {
        print!("{text}");
    }
    println!("{status}");

    if !status.is_success() {
        bail!("replay failed: {status}");
    }
    Ok(())
}
