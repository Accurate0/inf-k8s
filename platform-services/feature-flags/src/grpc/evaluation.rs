use crate::convert;
use crate::engine::{Engine, EvalContext, Resolution};
use crate::model::ValueType;
use crate::pb;
use crate::pb::evaluation_server::Evaluation;
use crate::snapshot::SnapshotManager;
use futures::stream::StreamExt;
use serde_json::Value as Json;
use std::pin::Pin;
use std::sync::Arc;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;
use tonic::{Request, Response, Status};

pub struct EvaluationService {
    mgr: Arc<SnapshotManager>,
}

impl EvaluationService {
    pub fn new(mgr: Arc<SnapshotManager>) -> Self {
        Self { mgr }
    }

    fn resolve(&self, req: pb::ResolveRequest) -> (Engine, EvalContext, String) {
        let ctx = req.context.unwrap_or_default().into();
        (self.mgr.engine(), ctx, req.flag_key)
    }
}

/// Identity of the calling service, sent by the feature-flag client in the `client-id`
/// gRPC metadata header. Required on every request so we can tell who is streaming or
/// evaluating; an absent or empty value is rejected.
fn client_id_of<T>(request: &Request<T>) -> Result<String, Status> {
    request
        .metadata()
        .get("client-id")
        .and_then(|v| v.to_str().ok())
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| Status::unauthenticated("missing client-id"))
}

/// Outcome of a typed resolution: either a successful value+meta, or just an error
/// meta (the caller-side default is supplied by the provider, not the backend).
enum Typed<T> {
    Ok(T, pb::ResolutionMeta),
    Err(pb::ResolutionMeta),
}

fn resolved<T>(
    engine: &Engine,
    flag_key: &str,
    ctx: &EvalContext,
    extract: impl Fn(&Json) -> Option<T>,
    type_name: &str,
) -> Typed<T> {
    match engine.evaluate(flag_key, ctx) {
        Ok(res) => match extract(&res.value) {
            Some(value) => Typed::Ok(value, pb::ResolutionMeta::from(&res)),
            None => Typed::Err(convert::type_mismatch(type_name)),
        },
        Err(e) => Typed::Err(convert::meta_err(e.code.as_str(), e.message)),
    }
}

type EventStream = Pin<Box<dyn futures::Stream<Item = Result<pb::Event, Status>> + Send>>;
type SnapshotStream =
    Pin<Box<dyn futures::Stream<Item = Result<pb::SnapshotResponse, Status>> + Send>>;

fn snapshot_response(snapshot: &crate::model::Snapshot) -> pb::SnapshotResponse {
    pb::SnapshotResponse {
        version: snapshot.version,
        flags: snapshot.flags.values().map(pb::Flag::from).collect(),
        segments: snapshot.segments.values().map(pb::Segment::from).collect(),
    }
}

#[tonic::async_trait]
impl Evaluation for EvaluationService {
    async fn resolve_boolean(
        &self,
        request: Request<pb::ResolveRequest>,
    ) -> Result<Response<pb::ResolveBooleanResponse>, Status> {
        let client_id = client_id_of(&request)?;
        tracing::debug!(client_id, "resolve");
        let (engine, ctx, flag_key) = self.resolve(request.into_inner());
        let (value, meta) = match resolved(&engine, &flag_key, &ctx, Json::as_bool, "boolean") {
            Typed::Ok(v, m) => (v, m),
            Typed::Err(m) => (false, m),
        };
        Ok(Response::new(pb::ResolveBooleanResponse {
            value,
            meta: Some(meta),
        }))
    }

    async fn resolve_string(
        &self,
        request: Request<pb::ResolveRequest>,
    ) -> Result<Response<pb::ResolveStringResponse>, Status> {
        let client_id = client_id_of(&request)?;
        tracing::debug!(client_id, "resolve");
        let (engine, ctx, flag_key) = self.resolve(request.into_inner());
        let (value, meta) = match resolved(
            &engine,
            &flag_key,
            &ctx,
            |j| j.as_str().map(str::to_owned),
            "string",
        ) {
            Typed::Ok(v, m) => (v, m),
            Typed::Err(m) => (String::new(), m),
        };
        Ok(Response::new(pb::ResolveStringResponse {
            value,
            meta: Some(meta),
        }))
    }

    async fn resolve_integer(
        &self,
        request: Request<pb::ResolveRequest>,
    ) -> Result<Response<pb::ResolveIntegerResponse>, Status> {
        let client_id = client_id_of(&request)?;
        tracing::debug!(client_id, "resolve");
        let (engine, ctx, flag_key) = self.resolve(request.into_inner());
        let (value, meta) = match resolved(&engine, &flag_key, &ctx, Json::as_i64, "integer") {
            Typed::Ok(v, m) => (v, m),
            Typed::Err(m) => (0, m),
        };
        Ok(Response::new(pb::ResolveIntegerResponse {
            value,
            meta: Some(meta),
        }))
    }

    async fn resolve_float(
        &self,
        request: Request<pb::ResolveRequest>,
    ) -> Result<Response<pb::ResolveFloatResponse>, Status> {
        let client_id = client_id_of(&request)?;
        tracing::debug!(client_id, "resolve");
        let (engine, ctx, flag_key) = self.resolve(request.into_inner());
        let (value, meta) = match resolved(&engine, &flag_key, &ctx, Json::as_f64, "float") {
            Typed::Ok(v, m) => (v, m),
            Typed::Err(m) => (0.0, m),
        };
        Ok(Response::new(pb::ResolveFloatResponse {
            value,
            meta: Some(meta),
        }))
    }

    async fn resolve_object(
        &self,
        request: Request<pb::ResolveRequest>,
    ) -> Result<Response<pb::ResolveObjectResponse>, Status> {
        let client_id = client_id_of(&request)?;
        tracing::debug!(client_id, "resolve");
        let (engine, ctx, flag_key) = self.resolve(request.into_inner());
        let extract = |j: &Json| j.as_object().map(|_| convert::json_to_struct(j));
        let (value, meta) = match resolved(&engine, &flag_key, &ctx, extract, "object") {
            Typed::Ok(v, m) => (Some(v), m),
            Typed::Err(m) => (None, m),
        };
        Ok(Response::new(pb::ResolveObjectResponse {
            value,
            meta: Some(meta),
        }))
    }

    async fn resolve_all(
        &self,
        request: Request<pb::ResolveAllRequest>,
    ) -> Result<Response<pb::ResolveAllResponse>, Status> {
        let client_id = client_id_of(&request)?;
        tracing::debug!(client_id, "resolve_all");
        let ctx: EvalContext = request.into_inner().context.unwrap_or_default().into();
        let engine = self.mgr.engine();
        let mut flags = Vec::new();
        for (key, flag) in &engine.snapshot().flags {
            if flag.archived {
                continue;
            }
            let evaluated = match engine.evaluate(key, &ctx) {
                Ok(res) => evaluated_flag(key, flag.value_type, &res),
                Err(e) => pb::EvaluatedFlag {
                    flag_key: key.clone(),
                    value_type: pb::ValueType::from(flag.value_type) as i32,
                    value: None,
                    meta: Some(convert::meta_err(e.code.as_str(), e.message)),
                },
            };
            flags.push(evaluated);
        }
        Ok(Response::new(pb::ResolveAllResponse { flags }))
    }

    async fn get_snapshot(
        &self,
        request: Request<pb::GetSnapshotRequest>,
    ) -> Result<Response<pb::SnapshotResponse>, Status> {
        client_id_of(&request)?;
        Ok(Response::new(snapshot_response(
            self.mgr.engine().snapshot(),
        )))
    }

    type StreamSnapshotStream = SnapshotStream;

    async fn stream_snapshot(
        &self,
        request: Request<pb::GetSnapshotRequest>,
    ) -> Result<Response<Self::StreamSnapshotStream>, Status> {
        let client_id = client_id_of(&request)?;
        tracing::info!(
            client_id,
            version = self.mgr.version(),
            "snapshot stream connected"
        );
        let rx = self.mgr.subscribe();
        let head_mgr = self.mgr.clone();
        let head =
            futures::stream::once(
                async move { Ok(snapshot_response(head_mgr.engine().snapshot())) },
            );
        let tail_mgr = self.mgr.clone();
        let tail = BroadcastStream::new(rx)
            .map(move |_| Ok(snapshot_response(tail_mgr.engine().snapshot())));
        Ok(Response::new(Box::pin(head.chain(tail))))
    }

    type StreamEventsStream = EventStream;

    async fn stream_events(
        &self,
        request: Request<pb::EventStreamRequest>,
    ) -> Result<Response<Self::StreamEventsStream>, Status> {
        let client_id = client_id_of(&request)?;
        tracing::info!(
            client_id,
            version = self.mgr.version(),
            "event stream connected"
        );
        let rx = self.mgr.subscribe();
        let ready = pb::Event {
            r#type: pb::EventType::Ready as i32,
            config_version: self.mgr.version(),
            changed_flag_keys: Vec::new(),
        };
        let head = futures::stream::once(async move { Ok(ready) });
        let tail_mgr = self.mgr.clone();
        let tail =
            BroadcastStream::new(rx).map(move |item| Ok(config_event(item, tail_mgr.version())));
        Ok(Response::new(Box::pin(head.chain(tail))))
    }
}

fn config_event(
    item: Result<crate::snapshot::ConfigUpdate, BroadcastStreamRecvError>,
    current_version: i64,
) -> pb::Event {
    match item {
        Ok(update) => pb::Event {
            r#type: pb::EventType::ConfigurationChanged as i32,
            config_version: update.version,
            changed_flag_keys: (*update.changed_flag_keys).clone(),
        },
        Err(BroadcastStreamRecvError::Lagged(_)) => pb::Event {
            r#type: pb::EventType::Resync as i32,
            config_version: current_version,
            changed_flag_keys: Vec::new(),
        },
    }
}

fn evaluated_flag(key: &str, value_type: ValueType, res: &Resolution) -> pb::EvaluatedFlag {
    pb::EvaluatedFlag {
        flag_key: key.to_owned(),
        value_type: pb::ValueType::from(value_type) as i32,
        value: Some(convert::json_to_prost_value(&res.value)),
        meta: Some(pb::ResolutionMeta::from(res)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::ConfigUpdate;

    #[test]
    fn update_maps_to_configuration_changed() {
        let event = config_event(
            Ok(ConfigUpdate {
                version: 7,
                changed_flag_keys: std::sync::Arc::new(vec!["a".to_owned()]),
            }),
            5,
        );
        assert_eq!(event.r#type, pb::EventType::ConfigurationChanged as i32);
        assert_eq!(event.config_version, 7);
        assert_eq!(event.changed_flag_keys, vec!["a"]);
    }

    #[test]
    fn lag_maps_to_resync_with_current_version() {
        let event = config_event(Err(BroadcastStreamRecvError::Lagged(99)), 5);
        assert_eq!(event.r#type, pb::EventType::Resync as i32);
        assert_eq!(event.config_version, 5);
        assert!(event.changed_flag_keys.is_empty());
    }
}
