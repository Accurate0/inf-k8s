use clap::Parser;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    aig::run(aig::Cli::parse()).await
}
