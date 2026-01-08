use anyhow::Context;
use clap::{Parser, Subcommand};
use config_catalog_jwt::generate_jwt;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    GenerateJwt {},
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let config = aws_config::load_from_env().await;
    let secrets_client = aws_sdk_secretsmanager::Client::new(&config);

    match args.command {
        Commands::GenerateJwt {} => {
            let jwt_secret = secrets_client
                .get_secret_value()
                .secret_id("config-catalog-jwt-secret")
                .send()
                .await?
                .secret_string
                .context("must have secret")?;

            let jwt = generate_jwt(
                jwt_secret.as_bytes(),
                "config-catalog-cli",
                "config-catalog",
            )?;

            println!("{jwt}")
        }
    }

    Ok(())
}
