use futures::StreamExt;
use kanidm_client::{KanidmClient, KanidmClientBuilder};
use kanidm_sync::{Condition, KanidmGroup, KanidmOAuth2Client, KanidmUser};
use kube::{
    runtime::controller::{Action, Controller},
    Api, Client,
};
use std::{sync::Arc, time::Duration};

mod group;
mod oauth2;
mod user;

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("kube error: {0}")]
    Kube(#[from] kube::Error),

    #[error("kanidm client error: {0}")]
    Kanidm(String),

    #[error("invalid redirect url: {0}")]
    Url(#[from] url::ParseError),

    #[error("kanidm returned no basic secret for client {0}")]
    MissingSecret(String),

    #[error("object {0} is missing a namespace")]
    MissingNamespace(String),

    #[error("configmap {0} has no key {1}")]
    MissingConfigMapKey(String, String),

    #[error("unsupported image type for {0}: {1}")]
    UnsupportedImageType(String, String),
}
pub type Result<T, E = Error> = std::result::Result<T, E>;

fn kanidm_err<E: std::fmt::Debug>(e: E) -> Error {
    Error::Kanidm(format!("{e:?}"))
}

struct ControllerContext {
    client: Client,
    kanidm: KanidmClient,
    kanidm_url: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().init();

    // Both ring and aws-lc-rs are pulled in transitively (kube vs reqwest), so rustls
    // cannot auto-select a provider — install one explicitly before any TLS is used.
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .expect("failed to install default rustls CryptoProvider");

    let kanidm_url = std::env::var("KANIDM_URL").expect("KANIDM_URL must be set");
    let kanidm_token = std::env::var("KANIDM_TOKEN").expect("KANIDM_TOKEN must be set");

    let kanidm = KanidmClientBuilder::new()
        .address(kanidm_url.clone())
        .build()
        .map_err(kanidm_err)?;
    kanidm.set_token(kanidm_token).await;

    tracing::info!("connected to kanidm at {kanidm_url}");

    let client = Client::try_default().await?;

    let ctx = ControllerContext {
        client: client.clone(),
        kanidm,
        kanidm_url,
    };
    let ctx = Arc::new(ctx);

    let oauth2_clients = Api::<KanidmOAuth2Client>::all(client.clone());
    let groups = Api::<KanidmGroup>::all(client.clone());
    let users = Api::<KanidmUser>::all(client.clone());

    // Controllers implemented in separate modules
    let oauth2_controller = {
        use crate::oauth2::reconcile as reconcile_oauth2_client;
        Controller::new(oauth2_clients, Default::default())
            .run(reconcile_oauth2_client, error_policy, ctx.clone())
            .for_each(|_| futures::future::ready(()))
    };

    let group_controller = {
        use crate::group::reconcile as reconcile_group;
        Controller::new(groups, Default::default())
            .run(reconcile_group, error_policy, ctx.clone())
            .for_each(|_| futures::future::ready(()))
    };

    let user_controller = {
        use crate::user::reconcile as reconcile_user;
        Controller::new(users, Default::default())
            .run(reconcile_user, error_policy, ctx.clone())
            .for_each(|_| futures::future::ready(()))
    };

    tokio::join!(oauth2_controller, group_controller, user_controller,);

    Ok(())
}

/// Build a condition, preserving `lastTransitionTime` when the status is unchanged.
fn condition(
    existing_conditions: Option<&Vec<Condition>>,
    type_: &str,
    status: &str,
    reason: &str,
    message: &str,
    observed_generation: i64,
) -> Condition {
    let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let last_transition_time = existing_conditions
        .and_then(|conds| conds.iter().find(|c| c.type_ == type_))
        .filter(|c| c.status == status)
        .map(|c| c.last_transition_time.clone())
        .unwrap_or(now);

    Condition {
        type_: type_.to_string(),
        status: status.to_string(),
        reason: reason.to_string(),
        message: message.to_string(),
        observed_generation,
        last_transition_time,
    }
}

fn error_policy<K>(_obj: Arc<K>, err: &Error, _ctx: Arc<ControllerContext>) -> Action {
    tracing::error!("reconcile error: {err}");
    Action::requeue(Duration::from_secs(60))
}
