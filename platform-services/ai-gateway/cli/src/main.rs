use clap::{CommandFactory, Parser};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    clap_complete::CompleteEnv::with_factory(aig::Cli::command).complete();
    aig::run(aig::Cli::parse()).await
}
