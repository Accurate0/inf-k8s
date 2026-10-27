use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "k8sctl",
    version,
    about = "inf-k8s platform services CLI",
    long_about = "inf-k8s platform services CLI. Each subcommand is the full standalone CLI \
                  for that service, with the same flags, env vars and stored login."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// feature-flags admin CLI and TUI (ffctl)
    #[command(name = "ff", visible_alias = "ffctl")]
    FeatureFlags(ffctl::Cli),

    /// ai-gateway admin CLI (aig)
    #[command(name = "ai", visible_alias = "aig")]
    AiGateway(aig::Cli),

    /// janitor-bot admin CLI (janitor)
    Janitor(janitor_cli::Cli),
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        Command::FeatureFlags(cli) => ffctl::run(cli).await,
        Command::AiGateway(cli) => aig::run(cli).await,
        Command::Janitor(cli) => janitor_cli::run(cli).await,
    }
}
