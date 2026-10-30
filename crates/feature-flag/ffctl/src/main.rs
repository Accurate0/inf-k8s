use clap::{CommandFactory, Parser};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    clap_complete::CompleteEnv::with_factory(ffctl::Cli::command).complete();
    ffctl::run(ffctl::Cli::parse()).await
}
