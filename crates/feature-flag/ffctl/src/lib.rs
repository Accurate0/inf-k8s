mod cli;
mod client;
mod commands;
mod convert;
mod editor;
mod flag_config;
mod output;
mod plan;
mod tui;
mod watch;

use clap::CommandFactory;
use cli::{Command, ConfigAction};
use cli_auth::{App, OAuthClient, TokenSource};
use client::{Api, Identity};
use commands::{Commands, ConfigDir};
use output::Output;
use std::io::IsTerminal;
use tui::Tui;

pub use cli::Cli;

const APP: App = App {
    name: "ffctl",
    scopes: "openid email profile",
    login_hint: "run `ffctl login`",
};

struct Session {
    url: String,
    oauth: OAuthClient,
    token: Option<String>,
    no_auth: bool,
    actor: Option<String>,
}

impl Session {
    fn new(cli: &Cli) -> Self {
        Self {
            url: cli.url.clone(),
            oauth: OAuthClient::new(APP, &cli.issuer, &cli.client_id),
            token: cli.token.clone().filter(|token| !token.is_empty()),
            no_auth: cli.no_auth,
            actor: cli.actor.clone(),
        }
    }

    async fn identity(&self) -> anyhow::Result<Identity> {
        if self.no_auth {
            return Ok(Identity::local_actor(self.actor.clone()));
        }

        if let Some(token) = &self.token {
            return Ok(Identity::Token(token.clone()));
        }

        Ok(Identity::Session(
            TokenSource::acquire(self.oauth.clone()).await?,
        ))
    }

    async fn api(&self) -> anyhow::Result<Api> {
        Api::connect(&self.url, self.identity().await?).await
    }
}

/// Runs ffctl. Shared by the `ffctl` binary and `k8sctl ff`.
pub async fn run(cli: Cli) -> anyhow::Result<()> {
    let session = Session::new(&cli);
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
        Command::Login => cli_auth::commands::login(&session.oauth).await,
        Command::Logout => cli_auth::commands::logout(&session.oauth),
        Command::Whoami => cli_auth::commands::whoami(&session.oauth, &session.url),
        Command::Config {
            action: ConfigAction::Schema { dir },
        } => ConfigDir::new(&dir).write_schema(),
        Command::Tui => Tui::run(session.api().await?).await,
        command => {
            Commands::new(session.api().await?, output)
                .run(command)
                .await
        }
    }
}
