use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use reqwest::Method;
use serde_json::Value;

#[derive(Parser)]
#[command(name = "janitor", about = "CLI for the janitor-bot admin API")]
struct Cli {
    /// Base URL of janitor-bot (e.g. http://localhost:3000)
    #[arg(
        long,
        global = true,
        env = "JANITOR_URL",
        default_value = "https://janitor-bot.inf-k8s.net"
    )]
    url: String,

    #[command(subcommand)]
    cmd: Command,
}

#[derive(Subcommand)]
enum Command {
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
}

#[derive(ValueEnum, Clone, Copy, Debug)]
enum CommandKind {
    Pr,
    Issue,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let client = reqwest::Client::new();
    let base = cli.url.trim_end_matches('/');

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
    };

    let url = format!("{base}{path}");
    let mut req = client.request(method, &url);
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
