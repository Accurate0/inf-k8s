//! Every platform CLI talks to its service through the federated graph, never its own REST API.
//! Each CLI's build.rs registers `platform-services/graph/supergraph/schema.graphql` as the
//! `graph` cynic schema, and its operations run through [`GraphClient`].

mod subscription;

use std::sync::Arc;

use anyhow::Context;
use cli_auth::{App, OAuthClient, TokenSource};
use cynic::{GraphQlError, GraphQlResponse, Operation, StreamingOperation};
use serde::Serialize;
use serde::de::DeserializeOwned;

pub use cynic;
pub use subscription::Subscription;

pub const DEFAULT_URL: &str = "https://graph.ortzi.cloud/v1/graphql";
pub const ISSUER: &str = "https://idm.anurag.sh/oauth2/openid/graph";
pub const CLIENT_ID: &str = "graph";

#[derive(Debug, Clone)]
pub struct Error {
    pub message: String,
    pub code: Option<String>,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.code.as_deref() {
            Some("UNAUTHENTICATED") => write!(f, "{} (log in again)", self.message),
            Some("FORBIDDEN") => write!(f, "{} (missing a group for this subgraph)", self.message),
            Some("RATE_LIMITED") => write!(f, "{} (rate limited; retry shortly)", self.message),
            _ => f.write_str(&self.message),
        }
    }
}

impl std::error::Error for Error {}

impl From<&GraphQlError<serde_json::Value>> for Error {
    fn from(error: &GraphQlError<serde_json::Value>) -> Self {
        let code = error
            .extensions
            .as_ref()
            .and_then(|e| e.get("code"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned);
        Self {
            message: error.message.clone(),
            code,
        }
    }
}

#[derive(Clone)]
enum Auth {
    Static(String),
    Session(TokenSource),
}

#[derive(Clone)]
pub struct GraphClient {
    http: reqwest::Client,
    url: Arc<str>,
    auth: Auth,
}

impl GraphClient {
    /// `GRAPH_URL` overrides the endpoint. `GRAPH_TOKEN`, which ortzi injects for packages with
    /// `auth.client_id: graph`, is used as is; otherwise the `app` session for the graph client
    /// is loaded, refreshed or logged into.
    pub async fn connect(app: App) -> anyhow::Result<Self> {
        let url = std::env::var("GRAPH_URL")
            .ok()
            .filter(|url| !url.is_empty())
            .unwrap_or_else(|| DEFAULT_URL.to_owned());

        let auth = match std::env::var("GRAPH_TOKEN").ok().filter(|t| !t.is_empty()) {
            Some(token) => Auth::Static(token),
            None => {
                Auth::Session(TokenSource::acquire(OAuthClient::new(app, ISSUER, CLIENT_ID)).await?)
            }
        };

        Ok(Self::with_auth(url, auth))
    }

    pub fn with_token(url: impl Into<String>, token: impl Into<String>) -> Self {
        Self::with_auth(url.into(), Auth::Static(token.into()))
    }

    fn with_auth(url: String, auth: Auth) -> Self {
        Self {
            http: reqwest::Client::builder()
                .user_agent(concat!("graph-client/", env!("CARGO_PKG_VERSION")))
                .build()
                .unwrap_or_default(),
            url: url.into(),
            auth,
        }
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    /// The logged-in user, or `token` when a bearer was handed in.
    pub fn user(&self) -> String {
        match &self.auth {
            Auth::Static(_) => "token".to_owned(),
            Auth::Session(source) => source.username(),
        }
    }

    pub(crate) async fn token(&self) -> anyhow::Result<String> {
        match &self.auth {
            Auth::Static(token) => Ok(token.clone()),
            Auth::Session(source) => {
                source.ensure_fresh().await?;
                Ok(source.access_token())
            }
        }
    }

    /// Runs a query or mutation; any GraphQL error fails the call, so partial data is never
    /// mistaken for a full answer.
    pub async fn run<T, V>(&self, operation: Operation<T, V>) -> anyhow::Result<T>
    where
        T: DeserializeOwned + 'static,
        V: Serialize,
    {
        let response = self
            .http
            .post(&*self.url)
            .bearer_auth(self.token().await?)
            .json(&operation)
            .send()
            .await
            .with_context(|| format!("calling {}", self.url))?;

        let status = response.status();
        let body = response.text().await?;
        let parsed: GraphQlResponse<T, serde_json::Value> = serde_json::from_str(&body)
            .with_context(|| format!("unexpected response from {} ({status}): {body}", self.url))?;

        if let Some(error) = parsed.errors.as_ref().and_then(|errors| errors.first()) {
            return Err(Error::from(error).into());
        }

        parsed
            .data
            .with_context(|| format!("{} returned no data ({status})", self.url))
    }

    /// Opens a graphql-transport-ws subscription on the same endpoint.
    pub async fn subscribe<T, V>(
        &self,
        operation: StreamingOperation<T, V>,
    ) -> anyhow::Result<Subscription<T>>
    where
        T: DeserializeOwned + Send + 'static,
        V: Serialize,
    {
        subscription::open(self, operation).await
    }
}

/// The code of the first GraphQL error behind `error`, e.g. `NOT_FOUND`.
pub fn error_code(error: &anyhow::Error) -> Option<&str> {
    error
        .downcast_ref::<Error>()
        .and_then(|e| e.code.as_deref())
}
