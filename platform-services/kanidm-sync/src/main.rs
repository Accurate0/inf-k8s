use futures::StreamExt;
use k8s_openapi::api::core::v1::{ConfigMap, Secret};
use kanidm_client::{KanidmClient, KanidmClientBuilder};
use kanidm_proto::internal::{ImageType, ImageValue, Oauth2ClaimMapJoin};
use kanidm_sync::{ClaimMapJoin, Condition, IconRef, KanidmOAuth2Client, KanidmOAuth2ClientStatus};
use kube::{
    api::{Patch, PatchParams},
    runtime::controller::{Action, Controller},
    Api, Client, Resource, ResourceExt,
};
use std::{collections::BTreeSet, sync::Arc, time::Duration};
use url::Url;

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
    let clients = Api::<KanidmOAuth2Client>::all(client.clone());

    let ctx = ControllerContext {
        client,
        kanidm,
        kanidm_url,
    };

    Controller::new(clients, Default::default())
        .run(reconcile, error_policy, Arc::new(ctx))
        .for_each(|_| futures::future::ready(()))
        .await;

    Ok(())
}

async fn reconcile(obj: Arc<KanidmOAuth2Client>, ctx: Arc<ControllerContext>) -> Result<Action> {
    let outcome = provision(&obj, &ctx).await;
    if let Err(e) = write_status(&obj, &ctx, outcome.as_ref().err()).await {
        tracing::warn!("failed to write status for {}: {e}", obj.name_any());
    }
    outcome?;
    Ok(Action::requeue(Duration::from_secs(3600)))
}

async fn provision(obj: &KanidmOAuth2Client, ctx: &ControllerContext) -> Result<()> {
    let spec = &obj.spec;
    let kanidm = &ctx.kanidm;
    let name = spec.name.as_str();

    tracing::info!("reconciling oauth2 client {name} ({})", obj.name_any());

    // 1. Ensure every group referenced by a scope or claim map exists.
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

    // 2. Ensure the OAuth2 resource server exists, then update its core attributes.
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

    // 3. Reconcile redirect origins (add desired, remove stale).
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

    // 4. Flags.
    if spec.prefer_short_username {
        kanidm
            .idm_oauth2_rs_prefer_short_username(name)
            .await
            .map_err(kanidm_err)?;
    }
    if spec.allow_insecure_client_disable_pkce {
        kanidm
            .idm_oauth2_rs_disable_pkce(name)
            .await
            .map_err(kanidm_err)?;
    }
    if spec.enable_legacy_crypto {
        kanidm
            .idm_oauth2_rs_enable_legacy_crypto(name)
            .await
            .map_err(kanidm_err)?;
    }

    // 5. Scope maps (desired ones; orphan removal of groups is left to the user in v1).
    for (group, scopes) in &spec.scope_maps {
        let scopes: Vec<&str> = scopes.iter().map(String::as_str).collect();
        kanidm
            .idm_oauth2_rs_update_scope_map(name, group, scopes)
            .await
            .map_err(kanidm_err)?;
    }

    // 5b. Claim maps (group membership -> custom OIDC claim values, e.g. an
    // "admin" value an app reads to auto-promote platform_admins members).
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

    // 5c. App icon (best-effort: a bad icon must never block auth provisioning).
    if let Some(icon) = &spec.icon {
        if let Err(e) = upload_icon(obj, ctx, name, icon).await {
            tracing::warn!("failed to set icon for {name}: {e}");
        }
    }

    // 6. For confidential clients, write the canonical Secret (id/secret/issuer).
    if !spec.public {
        let secret_value = kanidm
            .idm_oauth2_rs_get_basic_secret(name)
            .await
            .map_err(kanidm_err)?
            .ok_or_else(|| Error::MissingSecret(name.to_string()))?;

        write_secret(obj, ctx, &secret_value).await?;
    }

    Ok(())
}

/// Write back Accepted/Programmed conditions to the CR's status subresource,
/// mirroring the standard Kubernetes condition shape (like Gateway status).
async fn write_status(
    obj: &KanidmOAuth2Client,
    ctx: &ControllerContext,
    error: Option<&Error>,
) -> Result<()> {
    let namespace = obj
        .namespace()
        .ok_or_else(|| Error::MissingNamespace(obj.name_any()))?;
    let generation = obj.metadata.generation.unwrap_or(0);
    let existing = obj.status.as_ref();

    let (status, reason, message) = match error {
        None => (
            "True",
            "Programmed",
            "OAuth2 client provisioned in kanidm".to_string(),
        ),
        Some(e) => ("False", "ReconcileFailed", e.to_string()),
    };

    let conditions = vec![
        condition(
            existing,
            "Accepted",
            "True",
            "Accepted",
            "KanidmOAuth2Client has been accepted",
            generation,
        ),
        condition(existing, "Programmed", status, reason, &message, generation),
    ];

    let patch = serde_json::json!({ "status": { "conditions": conditions } });
    Api::<KanidmOAuth2Client>::namespaced(ctx.client.clone(), &namespace)
        .patch_status(
            &obj.name_any(),
            &PatchParams::default(),
            &Patch::Merge(&patch),
        )
        .await?;

    Ok(())
}

/// Build a condition, preserving `lastTransitionTime` when the status is unchanged.
fn condition(
    existing: Option<&KanidmOAuth2ClientStatus>,
    type_: &str,
    status: &str,
    reason: &str,
    message: &str,
    observed_generation: i64,
) -> Condition {
    let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let last_transition_time = existing
        .and_then(|s| s.conditions.iter().find(|c| c.type_ == type_))
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

async fn ensure_group(kanidm: &KanidmClient, group: &str) -> Result<()> {
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

    // Binary images (PNG/JPG/...) land in binaryData; SVG (text) in data.
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

fn join_proto(join: ClaimMapJoin) -> Oauth2ClaimMapJoin {
    match join {
        ClaimMapJoin::Array => Oauth2ClaimMapJoin::Array,
        ClaimMapJoin::Csv => Oauth2ClaimMapJoin::Csv,
        ClaimMapJoin::Ssv => Oauth2ClaimMapJoin::Ssv,
    }
}

async fn write_secret(
    obj: &KanidmOAuth2Client,
    ctx: &ControllerContext,
    client_secret: &str,
) -> Result<()> {
    let spec = &obj.spec;
    let issuer_url = format!("{}/oauth2/openid/{}", ctx.kanidm_url, spec.name);

    // Only own the Secret when it lives in the same namespace as the CR
    // (cross-namespace owner references are invalid).
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

    // Key names are configurable so each app can consume the secret as-is.
    let keys = &spec.secret_keys;
    let mut string_data = serde_json::Map::new();
    string_data.insert(keys.client_id.clone(), serde_json::json!(spec.name));
    string_data.insert(keys.client_secret.clone(), serde_json::json!(client_secret));
    string_data.insert(keys.issuer_url.clone(), serde_json::json!(issuer_url));

    // Server-side apply requires apiVersion/kind in the body, so build it explicitly.
    let patch = serde_json::json!({
        "apiVersion": "v1",
        "kind": "Secret",
        "metadata": metadata,
        "stringData": serde_json::Value::Object(string_data),
    });

    let secrets = Api::<Secret>::namespaced(ctx.client.clone(), &spec.secret_namespace);
    secrets
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

fn error_policy(
    _obj: Arc<KanidmOAuth2Client>,
    err: &Error,
    _ctx: Arc<ControllerContext>,
) -> Action {
    tracing::error!("reconcile error: {err}");
    Action::requeue(Duration::from_secs(60))
}
