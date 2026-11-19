use crate::{kanidm_err, ControllerContext, Error, Reconcile, Result};
use k8s_openapi::api::core::v1::Secret;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use k8s_openapi::ByteString;
use kanidm_proto::internal::{ApiToken, ApiTokenPurpose};
use kanidm_sync::{ApiTokenSpec, Condition, KanidmServiceAccount};
use kube::{
    api::{Patch, PatchParams, PostParams},
    Api, Resource, ResourceExt,
};
use std::collections::BTreeMap;

const TOKEN_LABEL: &str = "kanidm-sync";
const TOKEN_KEY: &str = "token";

impl Reconcile for KanidmServiceAccount {
    const KIND: &'static str = "KanidmServiceAccount";
    const PROGRAMMED_OK: &'static str = "Service account provisioned in kanidm";
    const FINALIZER: &'static str = "kanidmserviceaccount.inf-k8s.net/cleanup";

    fn validate(&self) -> Result<(), String> {
        let spec = &self.spec;
        if spec.name.is_empty() {
            return Err("spec.name must not be empty".to_string());
        }
        if spec.entry_managed_by.is_empty() {
            return Err("spec.entryManagedBy must not be empty".to_string());
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
            .idm_service_account_get(name)
            .await
            .map_err(kanidm_err)?
            .is_none()
        {
            tracing::info!("creating service account {name}");
            kanidm
                .idm_service_account_create(name, &spec.display_name, &spec.entry_managed_by)
                .await
                .map_err(kanidm_err)?;
        }

        kanidm
            .idm_service_account_update(
                name,
                None,
                Some(&spec.display_name),
                Some(&spec.entry_managed_by),
                None,
            )
            .await
            .map_err(kanidm_err)?;

        let ours: Vec<ApiToken> = kanidm
            .idm_service_account_list_api_token(name)
            .await
            .map_err(kanidm_err)?
            .into_iter()
            .filter(|t| t.label == TOKEN_LABEL)
            .collect();

        let current = match &spec.api_token {
            Some(api_token) => Some(sync_token(self, ctx, api_token, &ours).await?),
            None => None,
        };

        for stale in ours
            .iter()
            .filter(|t| current.as_deref() != Some(t.token_id.to_string().as_str()))
        {
            tracing::info!("destroying stale api token {} on {name}", stale.token_id);
            kanidm
                .idm_service_account_destroy_api_token(name, stale.token_id)
                .await
                .map_err(kanidm_err)?;
        }

        Ok(())
    }

    async fn cleanup(&self, ctx: &ControllerContext) -> Result<()> {
        let kanidm = &ctx.kanidm;
        let name = self.spec.name.as_str();

        if kanidm
            .idm_service_account_get(name)
            .await
            .map_err(kanidm_err)?
            .is_some()
        {
            tracing::info!("deleting service account {name}");
            kanidm
                .idm_service_account_delete(name)
                .await
                .map_err(kanidm_err)?;
        }

        Ok(())
    }
}

/// API tokens can only be read when generated, so the Secret is the sole copy. A new token is
/// issued when the Secret or the token recorded in status is gone, or the purpose changed.
async fn sync_token(
    obj: &KanidmServiceAccount,
    ctx: &ControllerContext,
    spec: &ApiTokenSpec,
    ours: &[ApiToken],
) -> Result<String> {
    let name = obj.spec.name.as_str();
    let api = Api::<Secret>::namespaced(ctx.client.clone(), &spec.secret_namespace);
    let existing = api.get_opt(&spec.secret_name).await?;

    let held = existing
        .as_ref()
        .and_then(|s| s.data.as_ref())
        .and_then(|d| d.get(TOKEN_KEY))
        .and_then(|t| String::from_utf8(t.0.clone()).ok());
    let recorded = obj
        .status
        .as_ref()
        .and_then(|s| s.token_id.as_deref())
        .and_then(|id| ours.iter().find(|t| t.token_id.to_string() == id))
        .filter(|t| matches!(t.purpose, ApiTokenPurpose::ReadWrite) == spec.read_write);

    let (token_id, token) = match (recorded, held) {
        (Some(recorded), Some(held)) => (recorded.token_id.to_string(), held),
        _ => {
            tracing::info!("generating api token for service account {name}");
            let token = ctx
                .kanidm
                .idm_service_account_generate_api_token(
                    name,
                    TOKEN_LABEL,
                    None,
                    spec.read_write,
                    false,
                )
                .await
                .map_err(kanidm_err)?;

            let token_id = ctx
                .kanidm
                .idm_service_account_list_api_token(name)
                .await
                .map_err(kanidm_err)?
                .into_iter()
                .filter(|t| t.label == TOKEN_LABEL)
                .filter(|t| !ours.iter().any(|o| o.token_id == t.token_id))
                .max_by_key(|t| t.issued_at)
                .ok_or_else(|| Error::Kanidm(format!("issued api token for {name} not listed")))?
                .token_id
                .to_string();

            (token_id, token)
        }
    };

    write_secret(obj, ctx, spec, existing, &token).await?;

    let cr_namespace = obj
        .namespace()
        .ok_or_else(|| Error::MissingNamespace(obj.name_any()))?;
    let patch = serde_json::json!({ "status": { "tokenId": token_id } });
    Api::<KanidmServiceAccount>::namespaced(ctx.client.clone(), &cr_namespace)
        .patch_status(
            &obj.name_any(),
            &PatchParams::default(),
            &Patch::Merge(&patch),
        )
        .await?;

    Ok(token_id)
}

async fn write_secret(
    obj: &KanidmServiceAccount,
    ctx: &ControllerContext,
    spec: &ApiTokenSpec,
    existing: Option<Secret>,
    token: &str,
) -> Result<()> {
    let cr_namespace = obj
        .namespace()
        .ok_or_else(|| Error::MissingNamespace(obj.name_any()))?;
    let api = Api::<Secret>::namespaced(ctx.client.clone(), &spec.secret_namespace);

    let mut data = BTreeMap::from([
        (TOKEN_KEY.to_string(), token.to_string()),
        (
            "tokenUrl".to_string(),
            format!("{}/oauth2/token", ctx.kanidm_url),
        ),
    ]);
    if let Some(audience) = &spec.audience {
        data.insert("audience".to_string(), audience.clone());
    }
    if !spec.scopes.is_empty() {
        data.insert("scope".to_string(), spec.scopes.join(" "));
    }

    let mut secret = Secret {
        metadata: ObjectMeta {
            name: Some(spec.secret_name.clone()),
            namespace: Some(spec.secret_namespace.clone()),
            owner_references: (cr_namespace == spec.secret_namespace)
                .then(|| vec![obj.controller_owner_ref(&()).unwrap()]),
            ..Default::default()
        },
        data: Some(
            data.into_iter()
                .map(|(k, v)| (k, ByteString(v.into_bytes())))
                .collect(),
        ),
        ..Default::default()
    };

    match existing {
        Some(existing) => {
            if existing.data == secret.data {
                return Ok(());
            }
            secret.metadata.resource_version = existing.metadata.resource_version;
            api.replace(&spec.secret_name, &PostParams::default(), &secret)
                .await?;
        }
        None => {
            api.create(&PostParams::default(), &secret).await?;
        }
    }

    tracing::info!(
        "wrote secret {}/{}",
        spec.secret_namespace,
        spec.secret_name
    );

    Ok(())
}
