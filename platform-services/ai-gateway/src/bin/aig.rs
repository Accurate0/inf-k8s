use clap::{Parser, Subcommand};
use serde_json::json;

/// Admin CLI for ai-gateway. Talks to the `/admin/*` endpoints; point `--url` at a
/// port-forwarded gateway and supply the admin token.
#[derive(Parser)]
#[command(name = "aig", about = "ai-gateway admin CLI")]
struct Cli {
    #[arg(long, env = "AIG_URL", default_value = "http://localhost:3000")]
    url: String,
    #[arg(long, env = "AIG_ADMIN_TOKEN")]
    token: String,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Manage virtual keys
    Keys {
        #[command(subcommand)]
        action: KeyAction,
    },
    /// Show the rolled-up usage summary
    Usage,
    /// List routable providers
    Models,
}

#[derive(Subcommand)]
enum KeyAction {
    /// Mint a new virtual key (the plaintext token is printed once)
    Create {
        #[arg(long)]
        name: String,
        /// Restrict the key to specific models; repeatable. Omit for any model.
        #[arg(long = "model")]
        models: Vec<String>,
        /// Optional monthly token budget.
        #[arg(long)]
        budget: Option<i64>,
    },
    /// List existing keys
    List,
    /// Revoke a key by id
    Revoke { id: String },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let http = reqwest::Client::new();
    let base = cli.url.trim_end_matches('/');

    let request = match &cli.command {
        Command::Keys { action } => match action {
            KeyAction::Create {
                name,
                models,
                budget,
            } => http.post(format!("{base}/admin/keys")).json(&json!({
                "name": name,
                "allowed_models": models,
                "monthly_token_budget": budget,
            })),
            KeyAction::List => http.get(format!("{base}/admin/keys")),
            KeyAction::Revoke { id } => http.delete(format!("{base}/admin/keys/{id}")),
        },
        Command::Usage => http.get(format!("{base}/admin/usage")),
        Command::Models => http.get(format!("{base}/v1/models")),
    };

    send(request.bearer_auth(&cli.token)).await
}

async fn send(request: reqwest::RequestBuilder) -> anyhow::Result<()> {
    let response = request.send().await?;
    let status = response.status();
    let body = response.text().await?;

    if !status.is_success() {
        anyhow::bail!("request failed ({status}): {body}");
    }

    if body.is_empty() {
        println!("ok ({status})");
    } else if let Ok(value) = serde_json::from_str::<serde_json::Value>(&body) {
        println!("{}", serde_json::to_string_pretty(&value)?);
    } else {
        println!("{body}");
    }

    Ok(())
}
