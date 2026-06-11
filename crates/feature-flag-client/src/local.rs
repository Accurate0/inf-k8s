//! In-process local evaluation. The client opens the backend's `StreamSnapshot` RPC,
//! holds the latest snapshot as an [`Engine`], and resolves flags without a network
//! round-trip per evaluation. The stream pushes a fresh snapshot on every config
//! change, so the local engine stays live off a single long-lived stream.

use crate::{Error, Resolution};
use feature_flag_engine::{Engine, EvalContext, Snapshot, convert};
use feature_flag_proto::evaluation_client::EvaluationClient;
use feature_flag_proto::{
    EvaluatedFlag, EvaluationContext, GetSnapshotRequest, Reason, ResolutionMeta,
    ResolveAllResponse, SnapshotResponse, ValueType,
};
use serde_json::Value as Json;
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tonic::transport::Channel;

pub(crate) struct LocalEvaluator {
    engine: RwLock<Engine>,
}

impl LocalEvaluator {
    pub(crate) async fn bootstrap(client: EvaluationClient<Channel>) -> Result<Arc<Self>, Error> {
        let mut stream = open_stream(client.clone()).await?;
        let first = stream
            .message()
            .await?
            .ok_or_else(|| Error::Snapshot("snapshot stream closed before first message".into()))?;

        let evaluator = Arc::new(Self {
            engine: RwLock::new(build_engine(first)?),
        });
        tokio::spawn(refresh_loop(client, stream, evaluator.clone()));
        Ok(evaluator)
    }

    fn apply(&self, snapshot: SnapshotResponse) {
        match build_engine(snapshot) {
            Ok(engine) => *self.engine.write().unwrap() = engine,
            Err(e) => tracing::error!("ignoring invalid snapshot: {e}"),
        }
    }

    fn engine(&self) -> Engine {
        self.engine.read().unwrap().clone()
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

async fn refresh_loop(
    client: EvaluationClient<Channel>,
    mut stream: tonic::Streaming<SnapshotResponse>,
    evaluator: Arc<LocalEvaluator>,
) {
    loop {
        match stream.message().await {
            Ok(Some(snapshot)) => {
                tracing::info!("received new snapshot: {}", snapshot.version);
                evaluator.apply(snapshot);
                continue;
            }
            Ok(None) => {
                tracing::warn!("snapshot stream closed, reconnecting");
            }
            Err(e) => {
                tracing::warn!("snapshot stream error: {e}, reconnecting");
            }
        }

        // Reconnect only when stream closed or errored
        loop {
            tokio::time::sleep(Duration::from_secs(2)).await;
            match open_stream(client.clone()).await {
                Ok(s) => {
                    stream = s;
                    break;
                }
                Err(e) => tracing::warn!("snapshot stream reconnect failed: {e}"),
            }
        }
    }
}

async fn open_stream(
    mut client: EvaluationClient<Channel>,
) -> Result<tonic::Streaming<SnapshotResponse>, Error> {
    Ok(client
        .stream_snapshot(GetSnapshotRequest {})
        .await?
        .into_inner())
}

fn build_engine(snapshot: SnapshotResponse) -> Result<Engine, Error> {
    let snapshot = Snapshot::try_from(snapshot).map_err(|e| Error::Snapshot(e.to_string()))?;
    Ok(Engine::new(Arc::new(snapshot)))
}

fn error_resolution<T: Default>(code: &str) -> Resolution<T> {
    Resolution {
        value: T::default(),
        variant: String::new(),
        reason: Reason::Error,
        error_code: Some(code.to_string()),
    }
}
