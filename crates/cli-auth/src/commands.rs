//! The `login`, `logout` and `whoami` subcommands every CLI built on this crate exposes.

use crate::{Credentials, OAuthClient};

pub async fn login(oauth: &OAuthClient) -> anyhow::Result<()> {
    let credentials = oauth.login().await?;
    println!("Logged in as {}.", credentials.display_name());

    Ok(())
}

pub fn logout(oauth: &OAuthClient) -> anyhow::Result<()> {
    if Credentials::delete(oauth.app())? {
        println!("Logged out.");
    } else {
        println!("Not logged in.");
    }

    Ok(())
}

pub fn whoami(oauth: &OAuthClient, server: &str) -> anyhow::Result<()> {
    let Some(credentials) = Credentials::load(oauth.app(), oauth.issuer(), oauth.client_id())
    else {
        anyhow::bail!("not logged in; {}", oauth.app().login_hint);
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
    println!("server   {server}");
    println!("expires  {expires} ({renewable})");

    Ok(())
}
