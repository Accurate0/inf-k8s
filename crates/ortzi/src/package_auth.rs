use anyhow::Context;
use cli_auth::{App, OAuthClient, TokenSource};
use registry_api::PackageAuth;

use crate::store::Store;

fn leak(value: String) -> &'static str {
    Box::leak(value.into_boxed_str())
}

/// Packages on the graph client share ortzi's own login, so one `ortzi login` covers them all.
pub fn oauth(package: &str, auth: &PackageAuth) -> OAuthClient {
    if auth.client_id == graph_client::CLIENT_ID {
        return OAuthClient::new(crate::session::APP, &auth.issuer, &auth.client_id);
    }

    let app = App {
        name: leak(format!("ortzi/{package}")),
        scopes: leak(auth.scopes.clone()),
        login_hint: leak(format!("run `ortzi login {package}`")),
    };
    OAuthClient::new(app, &auth.issuer, &auth.client_id)
}

pub fn installed(package: &str) -> anyhow::Result<(String, PackageAuth)> {
    let installed = Store::open()?.load()?;
    let (name, package) = installed
        .resolve(package)
        .with_context(|| format!("{package} is not installed"))?;
    let auth = package
        .auth
        .clone()
        .with_context(|| format!("{name} doesn't declare any auth"))?;
    Ok((name.clone(), auth))
}

const GRAPH_AUDIENCE: &str = "graph.ortzi.cloud";

/// In forgejo actions a graph package sends the run's own id token, which the graph trusts as an
/// external issuer; everywhere else it's the user's kanidm login.
pub async fn token(package: &str, auth: &PackageAuth) -> anyhow::Result<String> {
    if auth.client_id == graph_client::CLIENT_ID
        && std::env::var_os("ACTIONS_ID_TOKEN_REQUEST_URL").is_some()
    {
        return crate::session::forgejo_id_token(GRAPH_AUDIENCE).await;
    }

    Ok(TokenSource::acquire(oauth(package, auth))
        .await?
        .access_token())
}
