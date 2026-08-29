use crate::error::Result;
use crate::metrics::Metrics;
use k8s_openapi::api::coordination::v1::{Lease, LeaseSpec};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::MicroTime;
use k8s_openapi::jiff::{SignedDuration, Timestamp};
use kube::api::{Api, ObjectMeta, PatchParams, PostParams};
use kube::{Client, ResourceExt};
use std::time::Duration;
use tokio::sync::watch;

const LEASE: &str = "waf-manager";
const DURATION_SECONDS: i32 = 15;
const RENEW: Duration = Duration::from_secs(5);
const RETRY: Duration = Duration::from_secs(2);

pub struct LeaderElector {
    api: Api<Lease>,
    identity: String,
    leader: watch::Sender<bool>,
}

impl LeaderElector {
    pub fn new(client: Client, namespace: &str, identity: String) -> Self {
        Self {
            api: Api::namespaced(client, namespace),
            identity,
            leader: watch::channel(false).0,
        }
    }

    pub fn subscribe(&self) -> watch::Receiver<bool> {
        self.leader.subscribe()
    }

    pub async fn run(&self) {
        Metrics::set_leader(false);

        loop {
            let held = match self.try_acquire().await {
                Ok(held) => held,
                Err(e) => {
                    tracing::warn!("lease acquisition failed: {e}");
                    false
                }
            };

            self.set_leader(held);

            if !held {
                tokio::time::sleep(RETRY).await;
                continue;
            }

            loop {
                tokio::time::sleep(RENEW).await;

                match self.try_acquire().await {
                    Ok(true) => continue,
                    Ok(false) => {
                        tracing::warn!("lease taken by another holder, standing down");
                        break;
                    }
                    Err(e) => {
                        tracing::warn!("lease renewal failed, standing down: {e}");
                        break;
                    }
                }
            }

            self.set_leader(false);
        }
    }

    pub async fn release(&self) {
        if !*self.leader.borrow() {
            return;
        }

        let patch = serde_json::json!({
            "spec": { "holderIdentity": null, "renewTime": null },
        });

        match self
            .api
            .patch(
                LEASE,
                &PatchParams::default(),
                &kube::api::Patch::Merge(&patch),
            )
            .await
        {
            Ok(_) => tracing::info!("released lease {LEASE}"),
            Err(e) => tracing::warn!("releasing lease failed: {e}"),
        }
    }

    async fn try_acquire(&self) -> Result<bool> {
        let now = Timestamp::now();

        let Some(existing) = self.api.get_opt(LEASE).await? else {
            let lease = Lease {
                metadata: ObjectMeta {
                    name: Some(LEASE.to_string()),
                    ..Default::default()
                },
                spec: Some(self.spec(now, 0)),
            };

            self.api.create(&PostParams::default(), &lease).await?;
            return Ok(true);
        };

        let spec = existing.spec.clone().unwrap_or_default();
        let holder = spec.holder_identity.as_deref();

        if holder.is_some_and(|h| h != self.identity) && !Self::is_expired(&spec, now) {
            return Ok(false);
        }

        let transitions = spec.lease_transitions.unwrap_or(0);
        let transitions = if holder == Some(self.identity.as_str()) {
            transitions
        } else {
            transitions + 1
        };

        let mut lease = Lease {
            metadata: ObjectMeta {
                name: Some(LEASE.to_string()),
                resource_version: existing.resource_version(),
                ..Default::default()
            },
            spec: Some(self.spec(now, transitions)),
        };

        lease.metadata.managed_fields = None;

        match self
            .api
            .replace(LEASE, &PostParams::default(), &lease)
            .await
        {
            Ok(_) => Ok(true),
            Err(kube::Error::Api(e)) if e.code == 409 => Ok(false),
            Err(e) => Err(e.into()),
        }
    }

    fn spec(&self, now: Timestamp, transitions: i32) -> LeaseSpec {
        LeaseSpec {
            holder_identity: Some(self.identity.clone()),
            lease_duration_seconds: Some(DURATION_SECONDS),
            acquire_time: Some(MicroTime(now)),
            renew_time: Some(MicroTime(now)),
            lease_transitions: Some(transitions),
            ..Default::default()
        }
    }

    fn is_expired(spec: &LeaseSpec, now: Timestamp) -> bool {
        let Some(renewed) = spec.renew_time.as_ref() else {
            return true;
        };

        let duration = spec.lease_duration_seconds.unwrap_or(DURATION_SECONDS);
        renewed.0 + SignedDuration::from_secs(duration.into()) < now
    }

    fn set_leader(&self, held: bool) {
        if *self.leader.borrow() == held {
            return;
        }

        if held {
            tracing::info!("acquired lease {LEASE} as {}", self.identity);
        } else {
            tracing::warn!("lost lease {LEASE}");
        }

        Metrics::set_leader(held);
        let _ = self.leader.send(held);
    }
}
