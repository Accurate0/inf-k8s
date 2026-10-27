use clap::Parser;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    janitor_cli::run(janitor_cli::Cli::parse()).await
}
