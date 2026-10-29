use clap::Parser;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    ffctl::run(ffctl::Cli::parse()).await
}
