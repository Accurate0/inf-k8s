mod cli;
mod client;
mod commands;
mod convert;
mod editor;
mod flag_config;
mod graphql;
mod output;
mod plan;
mod tui;
mod watch;

use clap::CommandFactory;
use cli::{Command, ConfigAction};
use cli_auth::App;
use client::Api;
use commands::{Commands, ConfigDir};
use graph_client::GraphClient;
use output::Output;
use std::io::IsTerminal;
use tui::Tui;

pub use cli::Cli;

const APP: App = App {
    name: "ortzi/ff",
    scopes: "openid email profile groups",
    login_hint: "run `ortzi login ff`",
};

async fn api() -> anyhow::Result<Api> {
    let graph = GraphClient::connect(APP).await?;
    let user = graph.user();
    Ok(Api::new(graph, user))
}

/// Runs ffctl, published as the `ff` ortzi package.
pub async fn run(cli: Cli) -> anyhow::Result<()> {
    let output = Output::new(cli.output);

    let command = match cli.command {
        Some(command) => command,
        None if std::io::stdout().is_terminal() => Command::Tui,
        None => {
            Cli::command().print_help()?;
            return Ok(());
        }
    };

    match command {
        Command::Config {
            action: ConfigAction::Schema { dir },
        } => ConfigDir::new(&dir).write_schema(),
        Command::Tui => Tui::run(api().await?).await,
        command => Commands::new(api().await?, output).run(command).await,
    }
}
