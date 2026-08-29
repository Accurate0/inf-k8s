use crate::allowlist::Allowlist;
use crate::compositor::{Compositor, Conflict};
use crate::crd::{Condition, WafBlock, WafPolicy};
use crate::error::{Error, Result};
use crate::metrics::Metrics;
use crate::policy::PolicyWriter;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::OwnerReference;
use kube::api::{Api, DeleteParams, DynamicObject, ListParams, Patch, PatchParams};
use kube::runtime::controller::Action;
use kube::{Client, Resource, ResourceExt};
use sqlx::postgres::PgPool;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

const REQUEUE: Duration = Duration::from_secs(3600);
const ERROR_REQUEUE: Duration = Duration::from_secs(30);

pub struct Context {
    pub client: Client,
    pub namespace: String,
    pub allowlist: Allowlist,
    pub writer: PolicyWriter,
    pub pool: PgPool,
    sync_lock: tokio::sync::Mutex<()>,
}

impl Context {
    pub fn new(
        client: Client,
        namespace: String,
        allowlist: Allowlist,
        writer: PolicyWriter,
        pool: PgPool,
    ) -> Self {
        Self {
            client,
            namespace,
            allowlist,
            writer,
            pool,
            sync_lock: tokio::sync::Mutex::new(()),
        }
    }

    pub fn blocks(&self) -> Api<WafBlock> {
        Api::namespaced(self.client.clone(), &self.namespace)
    }

    pub fn policies(&self) -> Api<WafPolicy> {
        Api::namespaced(self.client.clone(), &self.namespace)
    }

    pub async fn conflicts(&self) -> Result<Vec<Conflict>> {
        let rows = sqlx::query!("select source, message from conflicts order by id")
            .fetch_all(&self.pool)
            .await?;

        Ok(rows
            .into_iter()
            .map(|row| Conflict {
                source: row.source,
                message: row.message,
            })
            .collect())
    }

    async fn store_conflicts(&self, conflicts: &[Conflict]) -> Result<()> {
        let mut tx = self.pool.begin().await?;

        sqlx::query!("delete from conflicts")
            .execute(&mut *tx)
            .await?;

        for conflict in conflicts {
            sqlx::query!(
                "insert into conflicts (source, message) values ($1, $2)",
                conflict.source,
                conflict.message,
            )
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;

        Ok(())
    }

    pub async fn sync_all(&self) -> Result<Vec<Conflict>> {
        let _guard = self.sync_lock.lock().await;

        let blocks = self.blocks().list(&ListParams::default()).await?.items;
        let policies = self.policies().list(&ListParams::default()).await?.items;

        let mut gateways: BTreeSet<String> = BTreeSet::new();
        gateways.extend(policies.iter().map(|p| p.spec.gateway.clone()));
        gateways.extend(blocks.iter().map(|b| b.spec.gateway.clone()));
        gateways.extend(self.adopted_gateways().await?);

        let now = chrono::Utc::now();
        let protected = self.allowlist.entries().await;
        let mut all_conflicts = Vec::new();

        for gateway in gateways {
            let compositor = Compositor::new(&gateway);
            let cidrs = compositor.active_cidrs(&blocks, now, &protected);
            let for_gateway: Vec<WafPolicy> = policies
                .iter()
                .filter(|p| p.spec.gateway == gateway)
                .cloned()
                .collect();

            let compiled = compositor.compile(&for_gateway, &cidrs);
            self.writer.reconcile(&gateway, compiled.spec).await?;

            Metrics::set_active_blocks(&gateway, cidrs.len());
            all_conflicts.extend(compiled.conflicts);
        }

        Metrics::set_conflicts(all_conflicts.len());
        self.store_conflicts(&all_conflicts).await?;

        Ok(all_conflicts)
    }

    pub async fn ensure_owner(&self, obj: &WafBlock) -> Result<()> {
        if obj
            .metadata
            .owner_references
            .as_ref()
            .is_some_and(|refs| !refs.is_empty())
        {
            return Ok(());
        }

        let mut owner = self.policy_owner(&obj.spec.gateway).await?;
        owner.controller = Some(false);
        owner.block_owner_deletion = Some(false);

        let patch = serde_json::json!({ "metadata": { "ownerReferences": [owner] } });
        self.blocks()
            .patch(
                &obj.name_any(),
                &PatchParams::default(),
                &Patch::Merge(&patch),
            )
            .await?;

        tracing::info!("set owner on block {}", obj.name_any());
        Ok(())
    }

    async fn policy_owner(&self, gateway: &str) -> Result<OwnerReference> {
        let mut policies: Vec<WafPolicy> = self
            .policies()
            .list(&ListParams::default())
            .await?
            .items
            .into_iter()
            .filter(|p| p.spec.gateway == gateway)
            .collect();

        policies.sort_by(|a, b| {
            a.spec
                .priority
                .cmp(&b.spec.priority)
                .then_with(|| a.name_any().cmp(&b.name_any()))
        });

        Ok(policies
            .first()
            .and_then(|p| p.owner_ref(&()))
            .unwrap_or_else(|| self.writer.namespace_owner()))
    }
    async fn adopted_gateways(&self) -> Result<Vec<String>> {
        let api: Api<DynamicObject> = self.writer.api();

        let existing = api.list(&ListParams::default()).await?;
        Ok(existing
            .items
            .iter()
            .filter_map(|obj| {
                let annotations = obj.metadata.annotations.as_ref()?;
                if annotations
                    .get("inf-k8s.net/managed-by")
                    .map(String::as_str)
                    != Some(crate::policy::FIELD_MANAGER)
                {
                    return None;
                }

                annotations.get("inf-k8s.net/gateway").cloned()
            })
            .collect())
    }
}

pub async fn reconcile_block(obj: Arc<WafBlock>, ctx: Arc<Context>) -> Result<Action> {
    let now = chrono::Utc::now();

    if obj.is_expired(now) {
        tracing::info!("block {} expired, deleting", obj.name_any());
        ctx.blocks()
            .delete(&obj.name_any(), &DeleteParams::default())
            .await?;

        return Ok(Action::await_change());
    }

    if let Err(e) = ctx.ensure_owner(&obj).await {
        tracing::warn!("failed to set owner on {}: {e}", obj.name_any());
    }

    let checked = ctx.allowlist.parse_and_check(&obj.spec.cidr).await;

    if let Err(Error::ProtectedRange(cidr, why)) = &checked {
        tracing::warn!("deleting block {}: {cidr} overlaps {why}", obj.name_any());
        Metrics::record_block_rejected("protected_range");

        ctx.blocks()
            .delete(&obj.name_any(), &DeleteParams::default())
            .await?;
        ctx.sync_all().await?;

        return Ok(Action::await_change());
    }

    let accepted = checked.map(|_| ()).map_err(|e| {
        Metrics::record_block_rejected("invalid_cidr");
        e.to_string()
    });

    let synced = match &accepted {
        Ok(()) => ctx.sync_all().await.map(|_| ()),
        Err(_) => Ok(()),
    };

    write_status(
        &ctx,
        &BlockStatusTarget(obj.clone()),
        &accepted,
        synced.as_ref().err(),
        &[],
    )
    .await;

    Metrics::record_sync(synced.is_ok());

    accepted.map_err(|msg| Error::InvalidCidr(obj.spec.cidr.clone(), msg))?;
    synced?;

    Ok(Action::requeue(requeue_for(&obj, now)))
}

pub async fn reconcile_policy(obj: Arc<WafPolicy>, ctx: Arc<Context>) -> Result<Action> {
    let synced = ctx.sync_all().await;
    let mine: Vec<Conflict> = synced
        .as_ref()
        .map(|conflicts| {
            conflicts
                .iter()
                .filter(|c| c.source == obj.name_any())
                .cloned()
                .collect()
        })
        .unwrap_or_default();

    write_status(
        &ctx,
        &PolicyStatusTarget(obj.clone()),
        &Ok(()),
        synced.as_ref().err(),
        &mine,
    )
    .await;

    Metrics::record_sync(synced.is_ok());
    synced?;

    Ok(Action::requeue(REQUEUE))
}

pub fn block_error_policy(_obj: Arc<WafBlock>, err: &Error, _ctx: Arc<Context>) -> Action {
    tracing::error!("waf block reconcile failed: {err}");
    Action::requeue(ERROR_REQUEUE)
}

pub fn policy_error_policy(_obj: Arc<WafPolicy>, err: &Error, _ctx: Arc<Context>) -> Action {
    tracing::error!("waf policy reconcile failed: {err}");
    Action::requeue(ERROR_REQUEUE)
}

fn requeue_for(obj: &WafBlock, now: chrono::DateTime<chrono::Utc>) -> Duration {
    let Some(raw) = obj.spec.expires_at.as_deref() else {
        return REQUEUE;
    };

    let Ok(at) = chrono::DateTime::parse_from_rfc3339(raw) else {
        return REQUEUE;
    };

    let remaining = at.with_timezone(&chrono::Utc) - now;
    remaining
        .to_std()
        .map(|d| d.min(REQUEUE) + Duration::from_secs(1))
        .unwrap_or(ERROR_REQUEUE)
}

trait StatusTarget: Send + Sync {
    fn name(&self) -> String;
    fn generation(&self) -> i64;
    fn existing(&self) -> Option<&Vec<Condition>>;
    fn patch(&self, ctx: &Context, body: serde_json::Value) -> BoxFuture<'_, Result<()>>;
}

type BoxFuture<'a, T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;

struct BlockStatusTarget(Arc<WafBlock>);
struct PolicyStatusTarget(Arc<WafPolicy>);

impl StatusTarget for BlockStatusTarget {
    fn name(&self) -> String {
        self.0.name_any()
    }

    fn generation(&self) -> i64 {
        self.0.metadata.generation.unwrap_or(0)
    }

    fn existing(&self) -> Option<&Vec<Condition>> {
        self.0.status.as_ref().map(|s| &s.conditions)
    }

    fn patch(&self, ctx: &Context, body: serde_json::Value) -> BoxFuture<'_, Result<()>> {
        let api = ctx.blocks();
        let name = self.name();
        Box::pin(async move {
            api.patch_status(&name, &PatchParams::default(), &Patch::Merge(&body))
                .await?;
            Ok(())
        })
    }
}

impl StatusTarget for PolicyStatusTarget {
    fn name(&self) -> String {
        self.0.name_any()
    }

    fn generation(&self) -> i64 {
        self.0.metadata.generation.unwrap_or(0)
    }

    fn existing(&self) -> Option<&Vec<Condition>> {
        self.0.status.as_ref().map(|s| &s.conditions)
    }

    fn patch(&self, ctx: &Context, body: serde_json::Value) -> BoxFuture<'_, Result<()>> {
        let api = ctx.policies();
        let name = self.name();
        Box::pin(async move {
            api.patch_status(&name, &PatchParams::default(), &Patch::Merge(&body))
                .await?;
            Ok(())
        })
    }
}

async fn write_status(
    ctx: &Context,
    target: &dyn StatusTarget,
    accepted: &std::result::Result<(), String>,
    sync_error: Option<&Error>,
    conflicts: &[Conflict],
) {
    let generation = target.generation();
    let existing = target.existing();

    let (accepted_status, accepted_reason, accepted_message) = match accepted {
        Ok(()) if conflicts.is_empty() => ("True", "Accepted", "accepted".to_string()),
        Ok(()) => (
            "False",
            "Conflict",
            conflicts
                .iter()
                .map(|c| c.message.as_str())
                .collect::<Vec<_>>()
                .join("; "),
        ),
        Err(msg) => ("False", "Rejected", msg.clone()),
    };

    let (enforced_status, enforced_reason, enforced_message) = match accepted {
        Err(_) => ("False", "NotAccepted", "not accepted".to_string()),
        Ok(()) => match sync_error {
            None => ("True", "Enforced", "security policy applied".to_string()),
            Some(e) => ("False", "SyncFailed", e.to_string()),
        },
    };

    let conditions = vec![
        Condition::new(
            existing,
            "Accepted",
            accepted_status,
            accepted_reason,
            accepted_message,
            generation,
        ),
        Condition::new(
            existing,
            "Enforced",
            enforced_status,
            enforced_reason,
            enforced_message,
            generation,
        ),
    ];

    let body = serde_json::json!({ "status": { "conditions": conditions } });
    match target.patch(ctx, body).await {
        Ok(()) => {}
        Err(Error::Kube(kube::Error::Api(e))) if e.code == 404 => {
            tracing::debug!("{} deleted before status was written", target.name());
        }
        Err(e) => tracing::warn!("failed to write status for {}: {e}", target.name()),
    }
}
