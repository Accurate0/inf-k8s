use crate::{kanidm_err, ControllerContext, Reconcile, Result};
use kanidm_sync::{Condition, KanidmUser};

impl Reconcile for KanidmUser {
    const KIND: &'static str = "KanidmUser";
    const PROGRAMMED_OK: &'static str = "User provisioned in kanidm";

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

        let mail = (!spec.mail.is_empty()).then_some(&spec.mail[..]);
        kanidm
            .idm_person_account_update(
                name,
                None,
                Some(&spec.display_name),
                spec.legal_name.as_deref(),
                mail,
            )
            .await
            .map_err(kanidm_err)?;

        Ok(())
    }
}
