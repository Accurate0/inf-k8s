use anyhow::Context;
use cli_auth::{App, Credentials, OAuthClient, TokenSource};
use serde::Deserialize;

use crate::client::Client;

pub const APP: App = App {
    name: "ortzi",
    scopes: "openid email profile groups",
    login_hint: "run `ortzi login` or set ORTZI_TOKEN",
};

#[derive(Debug, Clone, clap::Args)]
pub struct Settings {
    #[arg(
        long,
        global = true,
        env = "ORTZI_REGISTRY_URL",
        default_value = "https://registry.ortzi.cloud"
    )]
    pub registry: String,

    #[arg(
        long,
        global = true,
        env = "ORTZI_OIDC_ISSUER",
        default_value = "https://idm.anurag.sh/oauth2/openid/registry",
        hide_short_help = true
    )]
    pub issuer: String,

    #[arg(
        long,
        global = true,
        env = "ORTZI_OIDC_CLIENT_ID",
        default_value = "registry",
        hide_short_help = true
    )]
    pub client_id: String,

    #[arg(
        long,
        global = true,
        env = "ORTZI_TOKEN",
        hide_env_values = true,
        help = "Registry publish token to use instead of the stored login"
    )]
    pub token: Option<String>,

    #[arg(
        long,
        global = true,
        env = "ORTZI_AUDIENCE",
        default_value = "registry.ortzi.cloud",
        hide_short_help = true
    )]
    pub audience: String,
}

impl Settings {
    pub fn oauth(&self) -> OAuthClient {
        OAuthClient::new(APP, &self.issuer, &self.client_id)
    }

    fn token(&self) -> Option<String> {
        self.token.clone().filter(|t| !t.is_empty())
    }

    async fn user_bearer(&self) -> anyhow::Result<String> {
        Ok(TokenSource::acquire(self.oauth()).await?.access_token())
    }

    async fn ambient_reader(&self) -> anyhow::Result<Option<Client>> {
        if let Some(token) = self.token() {
            return Ok(Some(Client::new(&self.registry, token)));
        }
        if std::env::var_os("ACTIONS_ID_TOKEN_REQUEST_URL").is_some() {
            let token = forgejo_id_token(&self.audience).await?;
            return Ok(Some(Client::new(&self.registry, token)));
        }
        Ok(None)
    }

    pub async fn reader(&self) -> anyhow::Result<Client> {
        if let Some(client) = self.ambient_reader().await? {
            return Ok(client);
        }
        Ok(Client::new(&self.registry, self.user_bearer().await?))
    }

    pub async fn reader_without_login(&self) -> anyhow::Result<Option<Client>> {
        if let Some(client) = self.ambient_reader().await? {
            return Ok(Some(client));
        }
        let oauth = self.oauth();
        let Some(credentials) = Credentials::load(APP, oauth.issuer(), oauth.client_id()) else {
            return Ok(None);
        };
        let credentials = if credentials.is_fresh() {
            credentials
        } else if credentials.refresh_token.is_some() {
            oauth.refresh(&credentials).await?
        } else {
            return Ok(None);
        };
        Ok(Some(Client::new(&self.registry, credentials.access_token)))
    }

    pub async fn publisher(&self) -> anyhow::Result<(Client, &'static str)> {
        if let Some(token) = self.token() {
            return Ok((Client::new(&self.registry, token), "ORTZI_TOKEN"));
        }

        if std::env::var_os("ACTIONS_ID_TOKEN_REQUEST_URL").is_some() {
            let token = forgejo_id_token(&self.audience).await?;
            return Ok((Client::new(&self.registry, token), "forgejo actions oidc"));
        }

        Ok((
            Client::new(&self.registry, self.user_bearer().await?),
            "ortzi login",
        ))
    }
}

async fn forgejo_id_token(audience: &str) -> anyhow::Result<String> {
    #[derive(Deserialize)]
    struct IdToken {
        value: String,
    }

    let url = std::env::var("ACTIONS_ID_TOKEN_REQUEST_URL")?;
    let request_token = std::env::var("ACTIONS_ID_TOKEN_REQUEST_TOKEN").context(
        "ACTIONS_ID_TOKEN_REQUEST_TOKEN is unset; add `enable-openid-connect: true` to the workflow",
    )?;
    let separator = if url.contains('?') { '&' } else { '?' };

    let response = reqwest::Client::new()
        .get(format!("{url}{separator}audience={audience}"))
        .header(
            reqwest::header::AUTHORIZATION,
            format!("bearer {request_token}"),
        )
        .send()
        .await
        .context("requesting a forgejo actions id token")?;

    let status = response.status();
    if !status.is_success() {
        anyhow::bail!(
            "forgejo refused an id token ({status}): {}; is `enable-openid-connect: true` set on the workflow?",
            response.text().await.unwrap_or_default()
        );
    }

    Ok(response.json::<IdToken>().await?.value)
}
