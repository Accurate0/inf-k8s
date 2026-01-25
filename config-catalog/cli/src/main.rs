use anyhow::Context;
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use tokio::fs::read_to_string;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    GenerateJwt {},
    Store {
        #[arg(short, long)]
        namespace: String,
        #[arg(short, long)]
        object: String,
        #[arg(short, long)]
        file: String,
        #[arg(short, long)]
        source: Option<String>,
    },
    Get {
        #[arg(short, long)]
        namespace: String,
        #[arg(short, long)]
        object: String,
    },
}

const API_BASE: &str = "https://object-registry.inf-k8s.net/v1";

async fn generate_jwt(secrets_client: &aws_sdk_secretsmanager::Client) -> anyhow::Result<String> {
    let jwt_secret = secrets_client
        .get_secret_value()
        .secret_id("config-catalog-jwt-secret")
        .send()
        .await?
        .secret_string
        .context("must have secret")?;

    object_registry::generate_jwt(
        jwt_secret.as_bytes(),
        "config-catalog-cli",
        "config-catalog",
    )
    .map_err(Into::into)
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let config = aws_config::load_from_env().await;
    let secrets_client = aws_sdk_secretsmanager::Client::new(&config);
    let http_client = reqwest::ClientBuilder::new().build()?;

    match args.command {
        Commands::GenerateJwt {} => {
            let jwt = generate_jwt(&secrets_client).await?;
            println!("{jwt}")
        }
        Commands::Store {
            namespace,
            object,
            file,
            source,
        } => {
            let path = PathBuf::from(file);
            let file_contents = read_to_string(path).await?;
            let request = http_client
                .put(format!("{API_BASE}/{namespace}/{object}"))
                .body(file_contents)
                .bearer_auth(generate_jwt(&secrets_client).await?);

            let request = if let Some(source) = source {
                request.header("X-Config-Catalog-Source", source)
            } else {
                request
            };

            let response = request.send().await?.error_for_status()?;

            println!("{}", response.status());
        }
        Commands::Get { namespace, object } => {
            let response = http_client
                .get(format!("{API_BASE}/{namespace}/{object}"))
                .bearer_auth(generate_jwt(&secrets_client).await?)
                .send()
                .await?
                .error_for_status()?;

            let body = response.text().await?;

            println!("{body}");
        }
    }

    Ok(())
}
