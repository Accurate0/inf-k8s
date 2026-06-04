use crate::{kanidm_err, ControllerContext, Error, Reconcile, Result};
use k8s_openapi::api::core::v1::{ConfigMap, Secret};
use kanidm_proto::internal::{ImageType, ImageValue, Oauth2ClaimMapJoin};
use kanidm_sync::{ClaimMapJoin, Condition, IconRef, KanidmOAuth2Client};
use kube::{
    api::{Patch, PatchParams},
    Api, Resource, ResourceExt,
};
use std::collections::BTreeSet;
use url::Url;

impl Reconcile for KanidmOAuth2Client {
    const KIND: &'static str = "KanidmOAuth2Client";
    const PROGRAMMED_OK: &'static str = "OAuth2 client provisioned in kanidm";

    fn validate(&self) -> Result<(), String> {
        let spec = &self.spec;
        if spec.name.is_empty() {
            return Err("spec.name must not be empty".to_string());
        }
        if spec.redirect_urls.is_empty() {
            return Err("spec.redirectUrls must not be empty".to_string());
        }
        Url::parse(&spec.origin).map_err(|e| format!("spec.origin is not a valid URL: {e}"))?;
        for url in &spec.redirect_urls {
            Url::parse(url)
                .map_err(|e| format!("spec.redirectUrls entry {url:?} is invalid: {e}"))?;
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

        let groups: BTreeSet<&String> = spec
            .scope_maps
            .keys()
            .chain(
                spec.claim_maps
                    .values()
                    .flat_map(|m| m.values_by_group.keys()),
            )
            .collect();
        for group in groups {
            ensure_group(kanidm, group).await?;
        }

        if kanidm
            .idm_oauth2_rs_get(name)
            .await
            .map_err(kanidm_err)?
            .is_none()
        {
            tracing::info!("creating oauth2 resource server {name}");
            if spec.public {
                kanidm
                    .idm_oauth2_rs_public_create(name, &spec.display_name, &spec.origin)
                    .await
                    .map_err(kanidm_err)?;
            } else {
                kanidm
                    .idm_oauth2_rs_basic_create(name, &spec.display_name, &spec.origin)
                    .await
                    .map_err(kanidm_err)?;
            }
        }

        kanidm
            .idm_oauth2_rs_update(
                name,
                None,
                Some(&spec.display_name),
                Some(&spec.origin),
                false,
            )
            .await
            .map_err(kanidm_err)?;

        let entry = kanidm.idm_oauth2_rs_get(name).await.map_err(kanidm_err)?;
        let current: BTreeSet<String> = entry
            .as_ref()
            .and_then(|e| e.attrs.get("oauth2_rs_origin"))
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .collect();
        let desired: BTreeSet<String> = spec.redirect_urls.iter().cloned().collect();

        for added in desired.difference(&current) {
            tracing::info!("adding redirect origin {added}");
            kanidm
                .idm_oauth2_client_add_origin(name, &Url::parse(added)?)
                .await
                .map_err(kanidm_err)?;
        }

        for removed in current.difference(&desired) {
            tracing::info!("removing redirect origin {removed}");
            kanidm
                .idm_oauth2_client_remove_origin(name, &Url::parse(removed)?)
                .await
                .map_err(kanidm_err)?;
        }

        if spec.prefer_short_username {
            kanidm
                .idm_oauth2_rs_prefer_short_username(name)
                .await
                .map_err(kanidm_err)?;
        } else {
            kanidm
                .idm_oauth2_rs_prefer_spn_username(name)
                .await
                .map_err(kanidm_err)?;
        }

        if !spec.public {
            if spec.allow_insecure_client_disable_pkce {
                kanidm
                    .idm_oauth2_rs_disable_pkce(name)
                    .await
                    .map_err(kanidm_err)?;
            } else {
                kanidm
                    .idm_oauth2_rs_enable_pkce(name)
                    .await
                    .map_err(kanidm_err)?;
            }
        }

        if spec.enable_legacy_crypto {
            kanidm
                .idm_oauth2_rs_enable_legacy_crypto(name)
                .await
                .map_err(kanidm_err)?;
        } else {
            kanidm
                .idm_oauth2_rs_disable_legacy_crypto(name)
                .await
                .map_err(kanidm_err)?;
        }

        for (group, scopes) in &spec.scope_maps {
            let scopes: Vec<&str> = scopes.iter().map(String::as_str).collect();
            kanidm
                .idm_oauth2_rs_update_scope_map(name, group, scopes)
                .await
                .map_err(kanidm_err)?;
        }

        let current_scope_groups = scope_map_groups(entry.as_ref());
        for group in current_scope_groups.difference(&spec.scope_maps.keys().cloned().collect()) {
            tracing::info!("removing stale scope map for group {group}");
            kanidm
                .idm_oauth2_rs_delete_scope_map(name, group)
                .await
                .map_err(kanidm_err)?;
        }

        for (claim_name, claim_map) in &spec.claim_maps {
            kanidm
                .idm_oauth2_rs_update_claim_map_join(name, claim_name, join_proto(claim_map.join))
                .await
                .map_err(kanidm_err)?;
            for (group, values) in &claim_map.values_by_group {
                kanidm
                    .idm_oauth2_rs_update_claim_map(name, claim_name, group, values)
                    .await
                    .map_err(kanidm_err)?;
            }
        }

        let desired_claim_pairs: BTreeSet<(String, String)> = spec
            .claim_maps
            .iter()
            .flat_map(|(claim, map)| {
                map.values_by_group
                    .keys()
                    .map(move |g| (claim.clone(), g.clone()))
            })
            .collect();
        for (claim, group) in claim_map_pairs(entry.as_ref()).difference(&desired_claim_pairs) {
            tracing::info!("removing stale claim map {claim} for group {group}");
            kanidm
                .idm_oauth2_rs_delete_claim_map(name, claim, group)
                .await
                .map_err(kanidm_err)?;
        }

        if let Some(icon) = &spec.icon {
            if let Err(e) = upload_icon(self, ctx, name, icon).await {
                tracing::warn!("failed to set icon for {name}: {e}");
            }
        }

        if !spec.public {
            let secret_value = kanidm
                .idm_oauth2_rs_get_basic_secret(name)
                .await
                .map_err(kanidm_err)?
                .ok_or_else(|| Error::MissingSecret(name.to_string()))?;

            write_secret(self, ctx, &secret_value).await?;
        }

        Ok(())
    }
}

async fn ensure_group(kanidm: &kanidm_client::KanidmClient, group: &str) -> Result<()> {
    if kanidm
        .idm_group_get(group)
        .await
        .map_err(kanidm_err)?
        .is_none()
    {
        tracing::info!("creating referenced group {group}");
        kanidm
            .idm_group_create(group, None)
            .await
            .map_err(kanidm_err)?;
    }
    Ok(())
}

fn scope_map_groups(entry: Option<&kanidm_proto::v1::Entry>) -> BTreeSet<String> {
    entry
        .and_then(|e| e.attrs.get("oauth2_rs_scope_map"))
        .map(|values| {
            values
                .iter()
                .filter_map(|v| v.split(':').next())
                .map(short_group_name)
                .collect()
        })
        .unwrap_or_default()
}

fn claim_map_pairs(entry: Option<&kanidm_proto::v1::Entry>) -> BTreeSet<(String, String)> {
    entry
        .and_then(|e| e.attrs.get("oauth2_rs_claim_map"))
        .map(|values| {
            values
                .iter()
                .filter_map(|v| {
                    let mut parts = v.splitn(3, ':');
                    let claim = parts.next()?.trim();
                    let group = parts.next()?;
                    if claim.is_empty() {
                        return None;
                    }
                    Some((claim.to_string(), short_group_name(group)))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn short_group_name(spn: &str) -> String {
    spn.trim()
        .split('@')
        .next()
        .unwrap_or("")
        .trim()
        .to_string()
}

async fn upload_icon(
    obj: &KanidmOAuth2Client,
    ctx: &ControllerContext,
    name: &str,
    icon: &IconRef,
) -> Result<()> {
    let cr_namespace = obj
        .namespace()
        .ok_or_else(|| Error::MissingNamespace(obj.name_any()))?;
    let ns = icon.namespace.as_deref().unwrap_or(&cr_namespace);

    let cm = Api::<ConfigMap>::namespaced(ctx.client.clone(), ns)
        .get(&icon.config_map)
        .await?;

    let contents: Vec<u8> = cm
        .binary_data
        .as_ref()
        .and_then(|d| d.get(&icon.key))
        .map(|b| b.0.clone())
        .or_else(|| {
            cm.data
                .as_ref()
                .and_then(|d| d.get(&icon.key))
                .map(|s| s.clone().into_bytes())
        })
        .ok_or_else(|| Error::MissingConfigMapKey(icon.config_map.clone(), icon.key.clone()))?;

    let ext = icon.key.rsplit('.').next().unwrap_or_default();
    let filetype = ImageType::try_from(ext)
        .map_err(|_| Error::UnsupportedImageType(icon.key.clone(), ext.to_string()))?;

    ctx.kanidm
        .idm_oauth2_rs_update_image(
            name,
            ImageValue {
                filename: icon.key.clone(),
                filetype,
                contents,
            },
        )
        .await
        .map_err(kanidm_err)?;

    tracing::info!("set icon for {name} from {ns}/{}", icon.config_map);
    Ok(())
}

async fn write_secret(
    obj: &KanidmOAuth2Client,
    ctx: &ControllerContext,
    client_secret: &str,
) -> Result<()> {
    let spec = &obj.spec;
    let issuer_url = format!("{}/oauth2/openid/{}", ctx.kanidm_url, spec.name);

    let cr_namespace = obj
        .namespace()
        .ok_or_else(|| Error::MissingNamespace(obj.name_any()))?;

    let mut metadata = serde_json::json!({
        "name": spec.secret_name,
        "namespace": spec.secret_namespace,
    });

    if cr_namespace == spec.secret_namespace {
        metadata["ownerReferences"] = serde_json::json!([obj.controller_owner_ref(&()).unwrap()]);
    }
    if !spec.secret_labels.is_empty() {
        metadata["labels"] = serde_json::json!(spec.secret_labels);
    }

    let keys = &spec.secret_keys;
    let mut string_data = serde_json::Map::new();
    string_data.insert(keys.client_id.clone(), serde_json::json!(spec.name));
    string_data.insert(keys.client_secret.clone(), serde_json::json!(client_secret));
    string_data.insert(keys.issuer_url.clone(), serde_json::json!(issuer_url));

    let patch = serde_json::json!({
        "apiVersion": "v1",
        "kind": "Secret",
        "metadata": metadata,
        "stringData": serde_json::Value::Object(string_data),
    });

    Api::<Secret>::namespaced(ctx.client.clone(), &spec.secret_namespace)
        .patch(
            &spec.secret_name,
            &PatchParams::apply("kanidm-sync").force(),
            &Patch::Apply(patch),
        )
        .await?;

    tracing::info!(
        "wrote secret {}/{}",
        spec.secret_namespace,
        spec.secret_name
    );
    Ok(())
}

fn join_proto(join: ClaimMapJoin) -> Oauth2ClaimMapJoin {
    match join {
        ClaimMapJoin::Array => Oauth2ClaimMapJoin::Array,
        ClaimMapJoin::Csv => Oauth2ClaimMapJoin::Csv,
        ClaimMapJoin::Ssv => Oauth2ClaimMapJoin::Ssv,
    }
}
