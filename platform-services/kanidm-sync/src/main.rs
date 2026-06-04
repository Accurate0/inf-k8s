use futures::StreamExt;
use kanidm_client::{KanidmClient, KanidmClientBuilder};
use kanidm_sync::{Condition, KanidmGroup, KanidmOAuth2Client, KanidmUser};
use kube::{
    api::{Patch, PatchParams},
    core::NamespaceResourceScope,
    runtime::controller::{Action, Controller},
    Api, Client, Resource, ResourceExt,
};
use serde::{de::DeserializeOwned, Serialize};
use std::{fmt::Debug, sync::Arc, time::Duration};

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

    #[error("invalid spec: {0}")]
    Validation(String),
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

    let ctx = Arc::new(ControllerContext {
        client: client.clone(),
        kanidm,
        kanidm_url,
    });

    let oauth2_clients = Api::<KanidmOAuth2Client>::all(client.clone());
    let groups = Api::<KanidmGroup>::all(client.clone());
    let users = Api::<KanidmUser>::all(client.clone());

    let oauth2_controller = Controller::new(oauth2_clients, Default::default())
        .run(reconcile::<KanidmOAuth2Client>, error_policy, ctx.clone())
        .for_each(|_| futures::future::ready(()));

    let group_controller = Controller::new(groups, Default::default())
        .run(reconcile::<KanidmGroup>, error_policy, ctx.clone())
        .for_each(|_| futures::future::ready(()));

    let user_controller = Controller::new(users, Default::default())
        .run(reconcile::<KanidmUser>, error_policy, ctx.clone())
        .for_each(|_| futures::future::ready(()));

    tokio::join!(oauth2_controller, group_controller, user_controller);

    Ok(())
}

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

pub(crate) trait Reconcile:
    Resource<DynamicType = (), Scope = NamespaceResourceScope>
    + Clone
    + Debug
    + DeserializeOwned
    + Serialize
    + Send
    + Sync
{
    const KIND: &'static str;
    const PROGRAMMED_OK: &'static str;

    fn validate(&self) -> Result<(), String> {
        Ok(())
    }

    fn provision(
        &self,
        ctx: &ControllerContext,
    ) -> impl std::future::Future<Output = Result<()>> + Send;

    fn existing_conditions(&self) -> Option<&Vec<Condition>>;
}

async fn reconcile<K: Reconcile>(obj: Arc<K>, ctx: Arc<ControllerContext>) -> Result<Action> {
    let accepted = obj.validate();
    let provisioned = match accepted {
        Ok(()) => {
            tracing::info!("reconciling {} {}", K::KIND, obj.name_any());
            obj.provision(&ctx).await
        }
        Err(_) => Ok(()),
    };

    if let Err(e) = write_status(&*obj, &ctx, &accepted, provisioned.as_ref().err()).await {
        tracing::warn!("failed to write status for {}: {e}", obj.name_any());
    }

    accepted.map_err(Error::Validation)?;
    provisioned?;

    Ok(Action::requeue(Duration::from_secs(3600)))
}

async fn write_status<K: Reconcile>(
    obj: &K,
    ctx: &ControllerContext,
    accepted: &Result<(), String>,
    provision_error: Option<&Error>,
) -> Result<()> {
    let namespace = obj
        .namespace()
        .ok_or_else(|| Error::MissingNamespace(obj.name_any()))?;
    let generation = obj.meta().generation.unwrap_or(0);
    let existing = obj.existing_conditions();

    let (accepted_status, accepted_reason, accepted_message) = match accepted {
        Ok(()) => ("True", "Accepted", format!("{} has been accepted", K::KIND)),
        Err(msg) => ("False", "InvalidSpec", msg.clone()),
    };

    let (programmed_status, programmed_reason, programmed_message) = match accepted {
        Err(_) => ("False", "NotAccepted", "spec validation failed".to_string()),
        Ok(()) => match provision_error {
            None => ("True", "Programmed", K::PROGRAMMED_OK.to_string()),
            Some(e) => ("False", "ReconcileFailed", e.to_string()),
        },
    };

    let conditions = vec![
        condition(
            existing,
            "Accepted",
            accepted_status,
            accepted_reason,
            &accepted_message,
            generation,
        ),
        condition(
            existing,
            "Programmed",
            programmed_status,
            programmed_reason,
            &programmed_message,
            generation,
        ),
    ];

    let patch = serde_json::json!({ "status": { "conditions": conditions } });
    Api::<K>::namespaced(ctx.client.clone(), &namespace)
        .patch_status(
            &obj.name_any(),
            &PatchParams::default(),
            &Patch::Merge(&patch),
        )
        .await?;

    Ok(())
}

fn error_policy<K: Resource>(_obj: Arc<K>, err: &Error, _ctx: Arc<ControllerContext>) -> Action {
    tracing::error!("reconcile error: {err}");
    Action::requeue(Duration::from_secs(300))
}
