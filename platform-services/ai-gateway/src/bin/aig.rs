use clap::{Parser, Subcommand};
use serde_json::json;

/// Admin CLI for ai-gateway. Talks to the `/admin/*` endpoints; point `--url` at a
/// port-forwarded gateway and supply the admin token.
#[derive(Parser)]
#[command(name = "aig", about = "ai-gateway admin CLI")]
struct Cli {
    #[arg(
        long,
        env = "AIG_URL",
        default_value = "https://ai-gateway.inf-k8s.net"
    )]
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
    /// Manage model pricing
    Prices {
        #[command(subcommand)]
        action: PriceAction,
    },
}

#[derive(Subcommand)]
enum PriceAction {
    /// Fetch current prices from llm-prices.com and upsert them into the gateway
    Sync {
        #[arg(long, default_value = "https://www.llm-prices.com/current-v1.json")]
        source: String,
    },
}

/// One entry from llm-prices.com `current-v1.json`. Rates are USD per million tokens.
#[derive(serde::Deserialize)]
struct UpstreamPrice {
    id: String,
    input: Option<f64>,
    output: Option<f64>,
    input_cached: Option<f64>,
}

#[derive(serde::Deserialize)]
struct UpstreamPrices {
    prices: Vec<UpstreamPrice>,
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
    /// Update an existing key; only the flags you pass are changed
    Update {
        id: String,
        #[arg(long)]
        name: Option<String>,
        /// Replace the allowed-models list; repeatable.
        #[arg(long = "model")]
        models: Option<Vec<String>>,
        #[arg(long)]
        budget: Option<i64>,
        /// Revoke (`true`) or restore (`false`) the key.
        #[arg(long)]
        revoked: Option<bool>,
    },
    /// Revoke a key by id
    Revoke { id: String },
    /// Mint a fresh token for an existing key (the old one stops working immediately)
    Regenerate { id: String },
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
            KeyAction::Update {
                id,
                name,
                models,
                budget,
                revoked,
            } => {
                let mut body = serde_json::Map::new();
                if let Some(name) = name {
                    body.insert("name".into(), json!(name));
                }
                if let Some(models) = models {
                    body.insert("allowed_models".into(), json!(models));
                }
                if let Some(budget) = budget {
                    body.insert("monthly_token_budget".into(), json!(budget));
                }
                if let Some(revoked) = revoked {
                    body.insert("revoked".into(), json!(revoked));
                }
                http.patch(format!("{base}/admin/keys/{id}")).json(&body)
            }
            KeyAction::Revoke { id } => http.delete(format!("{base}/admin/keys/{id}")),
            KeyAction::Regenerate { id } => http.post(format!("{base}/admin/keys/{id}/regenerate")),
        },
        Command::Usage => http.get(format!("{base}/admin/usage")),
        Command::Models => http.get(format!("{base}/v1/models")),
        Command::Prices { action } => match action {
            PriceAction::Sync { source } => {
                let upstream: UpstreamPrices = http.get(source).send().await?.json().await?;
                let prices: Vec<_> = upstream
                    .prices
                    .into_iter()
                    .filter_map(|p| match (p.input, p.output) {
                        (Some(input), Some(output)) => Some(json!({
                            "id": p.id,
                            "input_usd_per_mtok": input,
                            "output_usd_per_mtok": output,
                            "cached_usd_per_mtok": p.input_cached,
                        })),
                        _ => None,
                    })
                    .collect();
                eprintln!("fetched {} priced models from {source}", prices.len());
                http.post(format!("{base}/admin/prices")).json(&prices)
            }
        },
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
