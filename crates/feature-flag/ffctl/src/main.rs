mod auth;
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

use auth::{Credentials, OAuthClient, TokenSource};
use clap::{CommandFactory, Parser};
use cli::{Cli, Command, ConfigAction};
use client::{Api, Identity};
use commands::{Commands, ConfigDir};
use output::Output;
use std::io::IsTerminal;
use tui::Tui;

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
            oauth: OAuthClient::new(&cli.issuer, &cli.client_id),
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
            anyhow::bail!("not logged in; run `ffctl login`");
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
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
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
        Command::Login => session.login().await,
        Command::Logout => session.logout(),
        Command::Whoami => session.whoami(),
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
