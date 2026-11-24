mod graphql;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use cli_auth::App;
use cynic::{MutationBuilder, QueryBuilder};
use graph_client::GraphClient;
use serde::Serialize;
use std::path::PathBuf;

use graphql::*;

const APP: App = App {
    name: "ortzi/janitor",
    scopes: "openid email profile groups",
    login_hint: "run `ortzi login janitor`",
};

#[derive(Parser)]
#[command(name = "janitor", about = "janitor-bot admin CLI, through the graph")]
pub struct Cli {
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
        pr_number: i32,
    },

    /// Explain what rules would match for a PR without executing actions
    DryRun {
        owner: String,
        repo: String,
        pr_number: i32,
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
        number: i32,
        /// Command body, e.g. "@janitor merge squash"
        body: String,
    },

    /// Show this replica's prometheus counters and histograms
    Metrics,

    /// Show the orchestrator evaluation log
    Logs,

    /// Show the compiled rule summary
    Rules,

    /// Deep health check (forgejo + github)
    Health,

    /// Replay a saved webhook payload through its handler, as if the source had sent it.
    /// The bot signs it itself; the event is inferred from the payload unless given. Prints
    /// the handler's HTTP status; what it did is in the bot's logs.
    Replay {
        #[arg(value_enum)]
        source: Source,
        /// Path to a file containing the raw JSON webhook body
        file: PathBuf,
        /// Override the inferred event (X-GitHub-Event / X-Forgejo-Event)
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
    Flux,
}

fn print(value: &impl Serialize) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

/// Runs janitor, published as the `janitor` ortzi package.
pub async fn run(cli: Cli) -> Result<()> {
    let graph = GraphClient::connect(APP).await?;
    let pr = |owner, repo, number| PrVariables {
        owner,
        repo,
        number,
    };

    match cli.cmd {
        Command::Cron => print(&graph.run(RunCron::build(())).await?.janitor_run_cron),
        Command::Evaluate {
            owner,
            repo,
            pr_number,
        } => print(
            &graph
                .run(EvaluatePr::build(pr(owner, repo, pr_number)))
                .await?
                .janitor_evaluate_pr,
        ),
        Command::DryRun {
            owner,
            repo,
            pr_number,
        } => print(
            &graph
                .run(DryRunQuery::build(pr(owner, repo, pr_number)))
                .await?
                .janitor_dry_run,
        ),
        Command::MergeQueued => print(
            &graph
                .run(MergeQueued::build(()))
                .await?
                .janitor_merge_queued,
        ),
        Command::Dispatch {
            kind,
            owner,
            repo,
            number,
            body,
        } => {
            let target = match kind {
                CommandKind::Pr => CommandTarget::Pr,
                CommandKind::Issue => CommandTarget::Issue,
            };
            let input = CommandInput {
                owner,
                repo,
                number,
                target,
                body,
            };
            print(
                &graph
                    .run(Dispatch::build(CommandVariables { input }))
                    .await?
                    .janitor_command,
            )
        }
        Command::Metrics => print(&graph.run(MetricsQuery::build(())).await?.janitor_metrics),
        Command::Logs => print(
            &graph
                .run(EvaluationsQuery::build(()))
                .await?
                .janitor_evaluations,
        ),
        Command::Rules => print(&graph.run(RulesQuery::build(())).await?.janitor_rules),
        Command::Health => {
            let services = graph.run(HealthQuery::build(())).await?.janitor_health;
            print(&services)?;
            if services.iter().any(|s| !s.healthy) {
                bail!("not every service is healthy");
            }
            Ok(())
        }
        Command::Replay {
            source,
            file,
            event,
            event_type,
        } => {
            let payload = std::fs::read_to_string(&file)
                .with_context(|| format!("reading {}", file.display()))?;
            let source = match source {
                Source::Forgejo => WebhookSource::Forgejo,
                Source::Github => WebhookSource::Github,
                Source::Flux => WebhookSource::Flux,
            };
            let status = graph
                .run(Replay::build(ReplayVariables {
                    source,
                    event,
                    event_type,
                    payload,
                }))
                .await?
                .janitor_replay_webhook;
            println!("{status}");
            if !(200..300).contains(&status) {
                bail!("replay failed: {status}");
            }
            Ok(())
        }
    }
}
