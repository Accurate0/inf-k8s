use crate::{kanidm_err, ControllerContext, Reconcile, Result};
use kanidm_sync::{Condition, KanidmGroup};

impl Reconcile for KanidmGroup {
    const KIND: &'static str = "KanidmGroup";
    const PROGRAMMED_OK: &'static str = "Group provisioned in kanidm";

    fn validate(&self) -> Result<(), String> {
        if self.spec.name.is_empty() {
            return Err("spec.name must not be empty".to_string());
        }
        Ok(())
    }

    fn existing_conditions(&self) -> Option<&Vec<Condition>> {
        self.status.as_ref().map(|s| &s.conditions)
    }

    async fn provision(&self, ctx: &ControllerContext) -> Result<()> {
        let spec = &self.spec;
        let kanidm = &ctx.kanidm;
        let name = spec.name.as_str();

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
}
