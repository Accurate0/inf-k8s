//! In-process local evaluation. The client opens the backend's `StreamSnapshot` RPC,
//! holds the latest snapshot as an [`Engine`], and resolves flags without a network
//! round-trip per evaluation. The stream pushes a fresh snapshot on every config
//! change, so the local engine stays live off a single long-lived stream.

use crate::{Error, IdentifiedChannel, Resolution};
use feature_flag_engine::{Engine, EvalContext, Snapshot, convert};
use feature_flag_proto::evaluation_client::EvaluationClient;
use feature_flag_proto::{
    EvaluatedFlag, EvaluationContext, GetSnapshotRequest, Reason, ResolutionMeta,
    ResolveAllResponse, SnapshotResponse, ValueType,
};
use arc_swap::ArcSwap;
use serde_json::Value as Json;
use std::sync::Arc;
use std::time::Duration;

pub(crate) struct LocalEvaluator {
    snapshot: ArcSwap<Snapshot>,
}

impl LocalEvaluator {
    pub(crate) async fn bootstrap(
        client: EvaluationClient<IdentifiedChannel>,
    ) -> Result<Arc<Self>, Error> {
        let (stream, evaluator) = match open_initial(&client).await {
            Ok((stream, first)) => {
                let evaluator = Arc::new(Self {
                    snapshot: ArcSwap::from(build_snapshot(first)?),
                });
                (Some(stream), evaluator)
            }
            Err(e) => {
                tracing::warn!(
                    "snapshot bootstrap failed ({e}), serving defaults and retrying in background"
                );
                let evaluator = Arc::new(Self {
                    snapshot: ArcSwap::from(empty_snapshot()),
                });
                (None, evaluator)
            }
        };
        tokio::spawn(refresh_loop(client, stream, evaluator.clone()));
        Ok(evaluator)
    }

    fn apply(&self, snapshot: SnapshotResponse) {
        match build_snapshot(snapshot) {
            Ok(s) => self.snapshot.store(s),
            Err(e) => tracing::error!("ignoring invalid snapshot: {e}"),
        }
    }

    fn engine(&self) -> Engine {
        Engine::new(self.snapshot.load_full())
    }

    pub(crate) fn resolve_bool(&self, flag_key: &str, ctx: EvaluationContext) -> Resolution<bool> {
        self.resolve(flag_key, ctx, Json::as_bool)
    }

    pub(crate) fn resolve_string(
        &self,
        flag_key: &str,
        ctx: EvaluationContext,
    ) -> Resolution<String> {
        self.resolve(flag_key, ctx, |j| j.as_str().map(str::to_owned))
    }

    pub(crate) fn resolve_int(&self, flag_key: &str, ctx: EvaluationContext) -> Resolution<i64> {
        self.resolve(flag_key, ctx, Json::as_i64)
    }

    pub(crate) fn resolve_float(&self, flag_key: &str, ctx: EvaluationContext) -> Resolution<f64> {
        self.resolve(flag_key, ctx, Json::as_f64)
    }

    pub(crate) fn resolve_object(
        &self,
        flag_key: &str,
        ctx: EvaluationContext,
    ) -> Resolution<prost_types::Struct> {
        self.resolve(flag_key, ctx, |j| {
            j.as_object().map(|_| convert::json_to_struct(j))
        })
    }

    fn resolve<T: Default>(
        &self,
        flag_key: &str,
        ctx: EvaluationContext,
        extract: impl Fn(&Json) -> Option<T>,
    ) -> Resolution<T> {
        let engine = self.engine();
        let eval_ctx = EvalContext::from(ctx);
        match engine.evaluate(flag_key, &eval_ctx) {
            Ok(res) => match extract(&res.value) {
                Some(value) => Resolution {
                    value,
                    variant: res.variant,
                    reason: Reason::from(res.reason),
                    error_code: None,
                },
                None => error_resolution("TYPE_MISMATCH"),
            },
            Err(e) => error_resolution(e.code.as_str()),
        }
    }

    pub(crate) fn resolve_all(&self, ctx: EvaluationContext) -> ResolveAllResponse {
        let engine = self.engine();
        let eval_ctx = EvalContext::from(ctx);
        let mut flags = Vec::new();
        for (key, flag) in &engine.snapshot().flags {
            if flag.archived {
                continue;
            }
            let value_type = ValueType::from(flag.value_type) as i32;
            let evaluated = match engine.evaluate(key, &eval_ctx) {
                Ok(res) => EvaluatedFlag {
                    flag_key: key.clone(),
                    value_type,
                    value: Some(convert::json_to_prost_value(&res.value)),
                    meta: Some(ResolutionMeta::from(&res)),
                },
                Err(e) => EvaluatedFlag {
                    flag_key: key.clone(),
                    value_type,
                    value: None,
                    meta: Some(convert::meta_err(e.code.as_str(), e.message)),
                },
            };
            flags.push(evaluated);
        }
        ResolveAllResponse { flags }
    }
}

const RECONNECT_MIN: Duration = Duration::from_secs(1);
const RECONNECT_MAX: Duration = Duration::from_secs(60);
const OPEN_TIMEOUT: Duration = Duration::from_secs(10);
const BOOTSTRAP_ATTEMPTS: u32 = 8;

async fn refresh_loop(
    client: EvaluationClient<IdentifiedChannel>,
    initial: Option<tonic::Streaming<SnapshotResponse>>,
    evaluator: Arc<LocalEvaluator>,
) {
    let mut stream = initial;
    let mut backoff = RECONNECT_MIN;

    loop {
        let mut active = match stream.take() {
            Some(s) => s,
            None => {
                let s = loop {
                    tokio::time::sleep(with_jitter(backoff)).await;
                    backoff = (backoff * 2).min(RECONNECT_MAX);
                    match tokio::time::timeout(OPEN_TIMEOUT, open_stream(client.clone())).await {
                        Ok(Ok(s)) => break s,
                        Ok(Err(e)) => tracing::warn!("snapshot stream reconnect failed: {e}"),
                        Err(_) => tracing::warn!("snapshot stream reconnect timed out"),
                    }
                };
                tracing::info!("snapshot stream reconnected");
                s
            }
        };

        backoff = RECONNECT_MIN;

        loop {
            match active.message().await {
                Ok(Some(snapshot)) => {
                    tracing::info!("received new snapshot: {}", snapshot.version);
                    evaluator.apply(snapshot);
                }
                Ok(None) => {
                    tracing::warn!("snapshot stream closed, reconnecting");
                    break;
                }
                Err(e) => {
                    tracing::warn!("snapshot stream error: {e}, reconnecting");
                    break;
                }
            }
        }
    }
}

fn empty_snapshot() -> Arc<Snapshot> {
    build_snapshot(SnapshotResponse::default()).expect("empty snapshot always builds")
}

fn with_jitter(d: Duration) -> Duration {
    let jitter = rand::random::<f64>() * 0.3 + 0.85;
    d.mul_f64(jitter)
}

async fn open_initial(
    client: &EvaluationClient<IdentifiedChannel>,
) -> Result<(tonic::Streaming<SnapshotResponse>, SnapshotResponse), Error> {
    let mut backoff = RECONNECT_MIN;
    let mut last_err = None;
    for attempt in 1..=BOOTSTRAP_ATTEMPTS {
        match open_stream(client.clone()).await {
            Ok(mut stream) => match stream.message().await {
                Ok(Some(first)) => return Ok((stream, first)),
                Ok(None) => {
                    last_err = Some(Error::Snapshot(
                        "snapshot stream closed before first message".into(),
                    ))
                }
                Err(e) => last_err = Some(e.into()),
            },
            Err(e) => last_err = Some(e),
        }
        if attempt < BOOTSTRAP_ATTEMPTS {
            tracing::warn!(attempt, "snapshot bootstrap failed, retrying");
            tokio::time::sleep(with_jitter(backoff)).await;
            backoff = (backoff * 2).min(RECONNECT_MAX);
        }
    }
    Err(last_err.unwrap_or_else(|| Error::Snapshot("snapshot bootstrap failed".into())))
}

async fn open_stream(
    mut client: EvaluationClient<IdentifiedChannel>,
) -> Result<tonic::Streaming<SnapshotResponse>, Error> {
    Ok(client
        .stream_snapshot(GetSnapshotRequest {})
        .await?
        .into_inner())
}

fn build_snapshot(snapshot: SnapshotResponse) -> Result<Arc<Snapshot>, Error> {
    let snapshot = Snapshot::try_from(snapshot).map_err(|e| Error::Snapshot(e.to_string()))?;
    Ok(Arc::new(snapshot))
}

fn error_resolution<T: Default>(code: &str) -> Resolution<T> {
    Resolution {
        value: T::default(),
        variant: String::new(),
        reason: Reason::Error,
        error_code: Some(code.to_string()),
    }
}
