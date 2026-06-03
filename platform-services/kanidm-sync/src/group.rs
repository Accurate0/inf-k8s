use crate::{condition, kanidm_err, ControllerContext, Error, Result};
use kanidm_sync::KanidmGroup;
use kube::{
    api::{Patch, PatchParams},
    runtime::controller::Action,
    Api, ResourceExt,
};
use std::{sync::Arc, time::Duration};

pub async fn reconcile(obj: Arc<KanidmGroup>, ctx: Arc<ControllerContext>) -> Result<Action> {
    let outcome = provision(&obj, &ctx).await;
    if let Err(e) = write_status(&obj, &ctx, outcome.as_ref().err()).await {
        tracing::warn!("failed to write status for {}: {e}", obj.name_any());
    }
    outcome?;
    Ok(Action::requeue(Duration::from_secs(3600)))
}

async fn provision(obj: &KanidmGroup, ctx: &ControllerContext) -> Result<()> {
    let spec = &obj.spec;
    let kanidm = &ctx.kanidm;
    let name = spec.name.as_str();

    tracing::info!("reconciling group {name} ({})", obj.name_any());

    if kanidm
        .idm_group_get(name)
        .await
        .map_err(kanidm_err)?
        .is_none()
    {
        tracing::info!("creating group {name}");
        let entry_managed_by = spec.entry_managed_by.as_deref();
        kanidm
            .idm_group_create(name, entry_managed_by)
            .await
            .map_err(kanidm_err)?;
    }

    if let Some(entry_managed_by) = &spec.entry_managed_by {
        kanidm
            .idm_group_set_entry_managed_by(name, entry_managed_by)
            .await
            .map_err(kanidm_err)?;
    }

    if !spec.members.is_empty() {
        let members: Vec<&str> = spec.members.iter().map(String::as_str).collect();
        kanidm
            .idm_group_set_members(name, &members)
            .await
            .map_err(kanidm_err)?;
    }

    Ok(())
}

async fn write_status(
    obj: &KanidmGroup,
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
            "Group provisioned in kanidm".to_string(),
        ),
        Some(e) => ("False", "ReconcileFailed", e.to_string()),
    };

    let conditions = vec![
        condition(
            existing,
            "Accepted",
            "True",
            "Accepted",
            "KanidmGroup has been accepted",
            generation,
        ),
        condition(existing, "Programmed", status, reason, &message, generation),
    ];

    let patch = serde_json::json!({ "status": { "conditions": conditions } });
    Api::<KanidmGroup>::namespaced(ctx.client.clone(), &namespace)
        .patch_status(
            &obj.name_any(),
            &PatchParams::default(),
            &Patch::Merge(&patch),
        )
        .await?;

    Ok(())
}
