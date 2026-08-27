use crate::error::{Error, Result};
use k8s_openapi::api::core::v1::Namespace;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::OwnerReference;
use kube::api::{Api, DeleteParams, DynamicObject, GroupVersionKind, Patch, PatchParams};
use kube::{Client, ResourceExt, discovery::ApiResource};
use serde_json::{Value, json};

pub const FIELD_MANAGER: &str = "waf-manager";

/// Writes the compiled SecurityPolicy. waf-manager owns these objects outright;
/// contributions arrive as WafPolicy CRs. Never commit one to git.
pub struct PolicyWriter {
    api: Api<DynamicObject>,
    resource: ApiResource,
    namespace: String,
    owner: OwnerReference,
}

impl PolicyWriter {
    /// Owned by waf-manager's Namespace: cluster-scoped, so a legal owner across
    /// namespaces. A namespaced owner would be treated as dangling and GC'd.
    pub async fn new(client: Client, namespace: &str, owner_namespace: &str) -> Result<Self> {
        let gvk = GroupVersionKind::gvk("gateway.envoyproxy.io", "v1alpha1", "SecurityPolicy");
        // Spelled out: a naive pluraliser gets "securitypolicy" wrong.
        let resource = ApiResource::from_gvk_with_plural(&gvk, "securitypolicies");

        let owner_ns = Api::<Namespace>::all(client.clone())
            .get(owner_namespace)
            .await?;
        let uid = owner_ns
            .uid()
            .ok_or_else(|| Error::MissingNamespace(owner_namespace.to_string()))?;

        let owner = OwnerReference {
            api_version: "v1".to_string(),
            kind: "Namespace".to_string(),
            name: owner_namespace.to_string(),
            uid,
            controller: Some(true),
            // Would require delete permission on the namespace's finalizers.
            block_owner_deletion: Some(false),
        };

        Ok(Self {
            api: Api::namespaced_with(client, namespace, &resource),
            resource,
            namespace: namespace.to_string(),
            owner,
        })
    }

    pub fn policy_name(gateway: &str) -> String {
        format!("waf-manager-{gateway}")
    }

    /// Enumerates what we have already written.
    pub fn api(&self) -> Api<DynamicObject> {
        self.api.clone()
    }

    /// Applies the spec, or deletes the policy when there is nothing to enforce.
    pub async fn reconcile(&self, gateway: &str, spec: Option<Value>) -> Result<bool> {
        let name = Self::policy_name(gateway);

        let Some(spec) = spec else {
            self.delete(&name).await?;
            return Ok(false);
        };

        let manifest = json!({
            "apiVersion": self.resource.api_version,
            "kind": self.resource.kind,
            "metadata": {
                "name": name,
                "namespace": self.namespace,
                "ownerReferences": [self.owner],
                "annotations": {
                    "inf-k8s.net/managed-by": FIELD_MANAGER,
                    "inf-k8s.net/gateway": gateway,
                },
            },
            "spec": spec,
        });

        let object: DynamicObject = serde_json::from_value(manifest)?;

        // force: no other legitimate author, so reclaiming from a stray edit is right.
        self.api
            .patch(
                &name,
                &PatchParams::apply(FIELD_MANAGER).force(),
                &Patch::Apply(&object),
            )
            .await?;

        tracing::info!("applied security policy {}/{name}", self.namespace);
        Ok(true)
    }

    async fn delete(&self, name: &str) -> Result<()> {
        match self.api.delete(name, &DeleteParams::default()).await {
            Ok(_) => {
                tracing::info!("deleted security policy {}/{name}", self.namespace);
                Ok(())
            }
            Err(kube::Error::Api(e)) if e.code == 404 => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}
