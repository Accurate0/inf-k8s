use crate::{kanidm_err, ControllerContext, Error, Reconcile, Result};
use kanidm_client::KanidmClient;
use kanidm_proto::v1::Entry;
use kanidm_sync::{AccountPolicy, Condition, KanidmGroup};

impl Reconcile for KanidmGroup {
    const KIND: &'static str = "KanidmGroup";
    const PROGRAMMED_OK: &'static str = "Group provisioned in kanidm";
    const FINALIZER: &'static str = "kanidmgroup.inf-k8s.net/cleanup";

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

        if spec.members.is_empty() {
            kanidm
                .idm_group_purge_members(name)
                .await
                .map_err(kanidm_err)?;
        } else {
            let members: Vec<&str> = spec.members.iter().map(String::as_str).collect();
            kanidm
                .idm_group_set_members(name, &members)
                .await
                .map_err(kanidm_err)?;
        }

        if let Some(policy) = &spec.account_policy {
            AccountPolicySync {
                kanidm,
                group: name,
            }
            .apply(policy)
            .await?;
        }

        Ok(())
    }

    async fn cleanup(&self, ctx: &ControllerContext) -> Result<()> {
        let kanidm = &ctx.kanidm;
        let name = self.spec.name.as_str();

        if kanidm
            .idm_group_get(name)
            .await
            .map_err(kanidm_err)?
            .is_some()
        {
            tracing::info!("deleting group {name}");
            kanidm.idm_group_delete(name).await.map_err(kanidm_err)?;
        }

        Ok(())
    }
}

struct AccountPolicySync<'a> {
    kanidm: &'a KanidmClient,
    group: &'a str,
}

impl AccountPolicySync<'_> {
    const CREDENTIAL_TYPE_MINIMUM: &'static str = "credential_type_minimum";
    const AUTH_SESSION_EXPIRY: &'static str = "authsession_expiry";
    const PRIVILEGE_EXPIRY: &'static str = "privilege_expiry";
    const PASSWORD_MINIMUM_LENGTH: &'static str = "auth_password_minimum_length";

    async fn apply(&self, policy: &AccountPolicy) -> Result<()> {
        let kanidm = self.kanidm;
        let group = self.group;

        let entry = kanidm
            .idm_group_get(group)
            .await
            .map_err(kanidm_err)?
            .ok_or_else(|| Error::Kanidm(format!("group {group} not found")))?;

        let enabled = entry
            .attrs
            .get("class")
            .is_some_and(|classes| classes.iter().any(|c| c == "account_policy"));

        if !enabled {
            tracing::info!("enabling account policy on group {group}");
            kanidm
                .group_account_policy_enable(group)
                .await
                .map_err(kanidm_err)?;
        }

        match policy.credential_type_minimum {
            Some(minimum) => kanidm
                .group_account_policy_credential_type_minimum_set(group, minimum.as_str())
                .await
                .map_err(kanidm_err)?,
            None => self.reset(&entry, Self::CREDENTIAL_TYPE_MINIMUM).await?,
        }

        match policy.auth_session_expiry {
            Some(expiry) => kanidm
                .group_account_policy_authsession_expiry_set(group, expiry)
                .await
                .map_err(kanidm_err)?,
            None => self.reset(&entry, Self::AUTH_SESSION_EXPIRY).await?,
        }

        match policy.privilege_expiry {
            Some(expiry) => kanidm
                .group_account_policy_privilege_expiry_set(group, expiry)
                .await
                .map_err(kanidm_err)?,
            None => self.reset(&entry, Self::PRIVILEGE_EXPIRY).await?,
        }

        match policy.password_minimum_length {
            Some(length) => kanidm
                .group_account_policy_password_minimum_length_set(group, length)
                .await
                .map_err(kanidm_err)?,
            None => self.reset(&entry, Self::PASSWORD_MINIMUM_LENGTH).await?,
        }

        Ok(())
    }

    async fn reset(&self, entry: &Entry, attr: &str) -> Result<()> {
        if !entry.attrs.contains_key(attr) {
            return Ok(());
        }

        tracing::info!("resetting {attr} on group {}", self.group);
        self.kanidm
            .idm_group_purge_attr(self.group, attr)
            .await
            .map_err(kanidm_err)
    }
}
