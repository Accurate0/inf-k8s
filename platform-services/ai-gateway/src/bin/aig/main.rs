mod auth;
mod usage;

use ai_gateway::usage::{UsageRow, Window};
use auth::{Credentials, OAuthClient, TokenSource};
use clap::{Parser, Subcommand};
use serde_json::json;
use usage::UsageTable;

#[derive(Parser)]
#[command(name = "aig", about = "ai-gateway admin CLI")]
struct Cli {
    #[arg(
        long,
        global = true,
        env = "AIG_URL",
        default_value = "https://ai-gateway.inf-k8s.net"
    )]
    url: String,

    #[arg(
        long,
        global = true,
        env = "AIG_OIDC_ISSUER",
        default_value = "https://idm.anurag.sh/oauth2/openid/ai-gateway",
        help = "OIDC issuer used to log in"
    )]
    issuer: String,

    #[arg(
        long,
        global = true,
        env = "AIG_OIDC_CLIENT_ID",
        default_value = "ai-gateway",
        help = "OAuth client id"
    )]
    client_id: String,

    #[arg(
        long,
        global = true,
        env = "AIG_ADMIN_TOKEN",
        hide_env_values = true,
        help = "Use this admin token instead of the stored login"
    )]
    token: Option<String>,

    #[command(subcommand)]
    command: Command,
}

struct Session {
    url: String,
    oauth: OAuthClient,
    token: Option<String>,
}

impl Session {
    fn new(cli: &Cli) -> Self {
        Self {
            url: cli.url.trim_end_matches('/').to_owned(),
            oauth: OAuthClient::new(&cli.issuer, &cli.client_id),
            token: cli.token.clone().filter(|token| !token.is_empty()),
        }
    }

    async fn bearer(&self) -> anyhow::Result<String> {
        if let Some(token) = &self.token {
            return Ok(token.clone());
        }

        Ok(TokenSource::acquire(&self.oauth).await?.access_token)
    }

    async fn login(&self) -> anyhow::Result<()> {
        let credentials = self.oauth.login().await?;
        println!("Logged in as {}.", credentials.display_name());

        Ok(())
    }

    fn logout(&self) -> anyhow::Result<()> {
        if Credentials::delete()? {
            println!("Logged out.");
        } else {
            println!("Not logged in.");
        }

        Ok(())
    }

    fn whoami(&self) -> anyhow::Result<()> {
        let Some(credentials) = Credentials::load(self.oauth.issuer(), self.oauth.client_id())
        else {
            anyhow::bail!("not logged in; run `aig login`");
        };

        let expires = chrono::DateTime::from_timestamp(credentials.expires_at, 0)
            .map(|at| at.with_timezone(&chrono::Local).to_rfc3339())
            .unwrap_or_default();

        let renewable = if credentials.refresh_token.is_some() {
            "renews automatically"
        } else {
            "no refresh token"
        };

        println!("user     {}", credentials.display_name());
        println!("issuer   {}", credentials.issuer);
        println!("server   {}", self.url);
        println!("expires  {expires} ({renewable})");

        Ok(())
    }

    fn usage_request(&self, http: &reqwest::Client, since: Window) -> reqwest::RequestBuilder {
        http.get(format!("{}/admin/usage?since={since}", self.url))
    }

    async fn usage(&self, http: &reqwest::Client, since: Window) -> anyhow::Result<()> {
        let request = self.usage_request(http, since);
        let (_, body) = fetch(request.bearer_auth(self.bearer().await?)).await?;
        let rows: Vec<UsageRow> = serde_json::from_str(&body)?;

        print!("{}", UsageTable::new(since, rows));

        Ok(())
    }
}

#[derive(Subcommand)]
enum Command {
    /// Log in through the browser and store the session
    Login,
    /// Forget the stored session
    Logout,
    /// Show the logged in user
    Whoami,
    /// Manage virtual keys
    Keys {
        #[command(subcommand)]
        action: KeyAction,
    },
    /// Show usage per key and model over a recent window
    Usage {
        #[arg(
            long,
            default_value_t,
            help = "How far back to look, e.g. 30m, 24h, 7d, 2w"
        )]
        since: Window,

        #[arg(long, help = "Print the raw JSON instead of a table")]
        json: bool,
    },
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
    let session = Session::new(&cli);
    let http = reqwest::Client::new();
    let base = session.url.as_str();

    let request = match &cli.command {
        Command::Login => return session.login().await,
        Command::Logout => return session.logout(),
        Command::Whoami => return session.whoami(),
        Command::Models => return send(http.get(format!("{base}/v1/models"))).await,
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
        Command::Usage { since, json: false } => return session.usage(&http, *since).await,
        Command::Usage { since, .. } => session.usage_request(&http, *since),
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

    send(request.bearer_auth(session.bearer().await?)).await
}

async fn fetch(request: reqwest::RequestBuilder) -> anyhow::Result<(reqwest::StatusCode, String)> {
    let response = request.send().await?;
    let status = response.status();
    let body = response.text().await?;

    if !status.is_success() {
        anyhow::bail!("request failed ({status}): {body}");
    }

    Ok((status, body))
}

async fn send(request: reqwest::RequestBuilder) -> anyhow::Result<()> {
    let (status, body) = fetch(request).await?;

    if body.is_empty() {
        println!("ok ({status})");
    } else if let Ok(value) = serde_json::from_str::<serde_json::Value>(&body) {
        println!("{}", serde_json::to_string_pretty(&value)?);
    } else {
        println!("{body}");
    }

    Ok(())
}
