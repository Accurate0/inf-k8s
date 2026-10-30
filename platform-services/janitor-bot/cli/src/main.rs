use clap::{CommandFactory, Parser};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    clap_complete::CompleteEnv::with_factory(janitor_cli::Cli::command).complete();
    janitor_cli::run(janitor_cli::Cli::parse()).await
}
