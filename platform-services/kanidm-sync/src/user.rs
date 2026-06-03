use crate::{condition, kanidm_err, ControllerContext, Error, Result};
use kanidm_sync::KanidmUser;
use kube::{
    api::{Patch, PatchParams},
    runtime::controller::Action,
    Api, ResourceExt,
};
use std::{sync::Arc, time::Duration};

pub async fn reconcile(obj: Arc<KanidmUser>, ctx: Arc<ControllerContext>) -> Result<Action> {
    let outcome = provision(&obj, &ctx).await;
    if let Err(e) = write_status(&obj, &ctx, outcome.as_ref().err()).await {
        tracing::warn!("failed to write status for {}: {e}", obj.name_any());
    }
    outcome?;
    Ok(Action::requeue(Duration::from_secs(3600)))
}

async fn provision(obj: &KanidmUser, ctx: &ControllerContext) -> Result<()> {
    let spec = &obj.spec;
    let kanidm = &ctx.kanidm;
    let name = spec.name.as_str();

    tracing::info!("reconciling user {name} ({})", obj.name_any());

    if kanidm
        .idm_person_account_get(name)
        .await
        .map_err(kanidm_err)?
        .is_none()
    {
        tracing::info!("creating person account {name}");
        kanidm
            .idm_person_account_create(name, &spec.display_name)
            .await
            .map_err(kanidm_err)?;
    }

    let mail_addresses: Vec<String> = spec.mail.clone();
    let mail_slice_opt: Option<&[String]> = if mail_addresses.is_empty() {
        None
    } else {
        Some(&mail_addresses[..])
    };
    kanidm
        .idm_person_account_update(
            name,
            None,
            Some(&spec.display_name),
            spec.legal_name.as_deref(),
            mail_slice_opt,
        )
        .await
        .map_err(kanidm_err)?;

    Ok(())
}

async fn write_status(
    obj: &KanidmUser,
    ctx: &ControllerContext,
    error: Option<&Error>,
) -> Result<()> {
    let namespace = obj
        .namespace()
        .ok_or_else(|| Error::MissingNamespace(obj.name_any()))?;
    let generation = obj.metadata.generation.unwrap_or(0);
    let existing = obj.status.as_ref().map(|s| &s.conditions);

    let (status, reason, message) = match error {
        None => (
            "True",
            "Programmed",
            "User provisioned in kanidm".to_string(),
        ),
        Some(e) => ("False", "ReconcileFailed", e.to_string()),
    };

    let conditions = vec![
        condition(
            existing,
            "Accepted",
            "True",
            "Accepted",
            "KanidmUser has been accepted",
            generation,
        ),
        condition(existing, "Programmed", status, reason, &message, generation),
    ];

    let patch = serde_json::json!({ "status": { "conditions": conditions } });
    Api::<KanidmUser>::namespaced(ctx.client.clone(), &namespace)
        .patch_status(
            &obj.name_any(),
            &PatchParams::default(),
            &Patch::Merge(&patch),
        )
        .await?;

    Ok(())
}
