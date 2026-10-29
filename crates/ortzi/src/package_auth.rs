use anyhow::Context;
use cli_auth::{App, OAuthClient, TokenSource};
use registry_api::PackageAuth;

use crate::store::Store;

fn leak(value: String) -> &'static str {
    Box::leak(value.into_boxed_str())
}

pub fn oauth(package: &str, auth: &PackageAuth) -> OAuthClient {
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

pub async fn token(package: &str, auth: &PackageAuth) -> anyhow::Result<String> {
    Ok(TokenSource::acquire(oauth(package, auth))
        .await?
        .access_token())
}
