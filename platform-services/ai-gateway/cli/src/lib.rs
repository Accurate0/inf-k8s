mod graphql;
mod usage;

use ai_gateway_api::{UsageRow, Window};
use clap::{Parser, Subcommand};
use cli_auth::App;
use cynic::{MutationBuilder, QueryBuilder};
use graph_client::GraphClient;
use serde::Serialize;
use usage::UsageTable;

use graphql::*;

const APP: App = App {
    name: "ortzi/ai",
    scopes: "openid email profile groups",
    login_hint: "run `ortzi login ai`",
};

#[derive(Parser)]
#[command(name = "aig", about = "ai-gateway admin CLI, through the graph")]
pub struct Cli {
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
    /// List routable models and their prices
    Models,
    /// Manage model pricing
    Prices {
        #[command(subcommand)]
        action: PriceAction,
    },
}

#[derive(Subcommand)]
enum PriceAction {
    /// Fetch current prices from llm-prices.com now; the gateway also syncs daily
    Sync,
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
        budget: Option<i32>,
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
        budget: Option<i32>,
        /// Revoke (`true`) or restore (`false`) the key.
        #[arg(long)]
        revoked: Option<bool>,
    },
    /// Revoke a key by id
    Revoke { id: String },
    /// Mint a fresh token for an existing key (the old one stops working immediately)
    Regenerate { id: String },
}

/// Keys print with their graph id; a bare uuid from older output still works.
fn key_id(id: String) -> cynic::Id {
    if id.contains(':') {
        cynic::Id::new(id)
    } else {
        cynic::Id::new(format!("aiGateway:key:{id}"))
    }
}

fn print(value: &impl Serialize) -> anyhow::Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

/// Runs aig, published as the `ai` ortzi package.
pub async fn run(cli: Cli) -> anyhow::Result<()> {
    let graph = GraphClient::connect(APP).await?;

    match cli.command {
        Command::Models => print(&graph.run(ModelsQuery::build(())).await?.ai_gateway_models),
        Command::Keys { action } => match action {
            KeyAction::Create {
                name,
                models,
                budget,
            } => {
                let input = CreateKeyInput {
                    name,
                    allowed_models: models,
                    monthly_token_budget: budget,
                };
                print(
                    &graph
                        .run(CreateKey::build(CreateKeyVariables { input }))
                        .await?
                        .ai_gateway_create_key,
                )
            }
            KeyAction::List => print(&graph.run(KeysQuery::build(())).await?.ai_gateway_keys),
            KeyAction::Update {
                id,
                name,
                models,
                budget,
                revoked,
            } => {
                let input = UpdateKeyInput {
                    name,
                    allowed_models: models,
                    monthly_token_budget: budget,
                    revoked,
                };
                print(
                    &graph
                        .run(UpdateKey::build(UpdateKeyVariables {
                            id: key_id(id),
                            input,
                        }))
                        .await?
                        .ai_gateway_update_key,
                )
            }
            KeyAction::Revoke { id } => print(
                &graph
                    .run(RevokeKey::build(KeyIdVariables { id: key_id(id) }))
                    .await?
                    .ai_gateway_revoke_key,
            ),
            KeyAction::Regenerate { id } => print(
                &graph
                    .run(RegenerateKey::build(KeyIdVariables { id: key_id(id) }))
                    .await?
                    .ai_gateway_regenerate_key,
            ),
        },
        Command::Usage { since, json } => {
            let rows = graph
                .run(UsageQuery::build(UsageVariables {
                    since: since.to_string(),
                }))
                .await?
                .ai_gateway_usage;
            if json {
                return print(&rows);
            }

            let rows = rows
                .into_iter()
                .map(|row| UsageRow {
                    key_name: row.key_name,
                    model: row.model,
                    requests: row.requests.into(),
                    cache_hits: row.cache_hits.into(),
                    input_tokens: row.input_tokens.into(),
                    output_tokens: row.output_tokens.into(),
                    cost_usd: row.cost_usd,
                })
                .collect();
            print!("{}", UsageTable::new(since, rows));
            Ok(())
        }
        Command::Prices {
            action: PriceAction::Sync,
        } => {
            let written = graph
                .run(SyncPrices::build(()))
                .await?
                .ai_gateway_sync_prices;
            println!("synced {written} model prices");
            Ok(())
        }
    }
}
