//! A thin async gRPC client for the feature-flags backend, for direct consumption by
//! Rust services. The OpenFeature provider is built on top of this. Wire types come
//! from [`feature_flag_proto`], re-exported here as [`proto`].
//!
//! Evaluation can run in two [modes](EvaluationMode), chosen at construction:
//! [`Remote`](EvaluationMode::Remote) issues an RPC per evaluation, while
//! [`Local`](EvaluationMode::Local) streams the snapshot from the server and evaluates
//! in-process, avoiding a round-trip per flag.

mod context;
mod local;

pub use context::Context;
pub use feature_flag_proto as proto;

use feature_flag_proto::admin_client::AdminClient;
use feature_flag_proto::evaluation_client::EvaluationClient;
use feature_flag_proto::{
    EvaluationContext, Event, Reason, ResolutionMeta, ResolveAllResponse, ResolveRequest,
};
use local::LocalEvaluator;
use std::sync::Arc;
use tonic::Streaming;
use tonic::metadata::{Ascii, MetadataValue};
use tonic::service::Interceptor;
use tonic::service::interceptor::InterceptedService;
use tonic::transport::Channel;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid endpoint: {0}")]
    InvalidEndpoint(String),
    #[error("invalid client id: {0}")]
    InvalidClientId(String),
    #[error("connection error: {0}")]
    Connect(#[from] tonic::transport::Error),
    #[error("rpc error: {0}")]
    Rpc(#[from] tonic::Status),
    #[error("invalid snapshot: {0}")]
    Snapshot(String),
}

/// Channel wrapped with the [`ClientIdInterceptor`], used by both the evaluation and
/// admin clients so every request carries the `client-id` metadata header.
pub type IdentifiedChannel = InterceptedService<Channel, ClientIdInterceptor>;

/// Injects the caller's `client-id` into the metadata of every outgoing request,
/// including streaming opens, so the backend can identify who is connected.
#[derive(Clone)]
pub struct ClientIdInterceptor {
    client_id: MetadataValue<Ascii>,
}

impl Interceptor for ClientIdInterceptor {
    fn call(
        &mut self,
        mut request: tonic::Request<()>,
    ) -> Result<tonic::Request<()>, tonic::Status> {
        request
            .metadata_mut()
            .insert("client-id", self.client_id.clone());
        Ok(request)
    }
}

/// Where flag evaluation happens.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EvaluationMode {
    /// One gRPC call to the backend per evaluation.
    #[default]
    Remote,
    /// Evaluate in-process against a server-streamed snapshot.
    Local,
}

/// A resolved flag value plus its evaluation metadata. `error_code` is set when the
/// backend reported an error (e.g. `FLAG_NOT_FOUND`, `TYPE_MISMATCH`); callers should
/// fall back to their own default in that case.
#[derive(Clone, Debug)]
pub struct Resolution<T> {
    pub value: T,
    pub variant: String,
    pub reason: Reason,
    pub error_code: Option<String>,
}

#[derive(Clone)]
pub struct FeatureFlagClient {
    evaluation: EvaluationClient<IdentifiedChannel>,
    admin: AdminClient<IdentifiedChannel>,
    local: Option<Arc<LocalEvaluator>>,
}

impl FeatureFlagClient {
    /// Connect in [remote](EvaluationMode::Remote) mode. `client_id` identifies the
    /// calling service and is sent on every request as the `client-id` header.
    pub async fn connect(
        endpoint: impl Into<String>,
        client_id: impl Into<String>,
    ) -> Result<Self, Error> {
        Self::connect_with(endpoint, client_id, EvaluationMode::Remote).await
    }

    /// Connect in the given mode. In [`Local`](EvaluationMode::Local) mode this awaits
    /// the first streamed snapshot so the returned client is immediately usable.
    pub async fn connect_with(
        endpoint: impl Into<String>,
        client_id: impl Into<String>,
        mode: EvaluationMode,
    ) -> Result<Self, Error> {
        let channel = Channel::from_shared(endpoint.into())
            .map_err(|e| Error::InvalidEndpoint(e.to_string()))?
            .connect_lazy();
        Self::from_channel(channel, client_id, mode).await
    }

    pub async fn from_channel(
        channel: Channel,
        client_id: impl Into<String>,
        mode: EvaluationMode,
    ) -> Result<Self, Error> {
        let client_id = client_id.into();
        let interceptor = ClientIdInterceptor {
            client_id: client_id
                .parse()
                .map_err(|_| Error::InvalidClientId(client_id))?,
        };
        let evaluation = EvaluationClient::with_interceptor(channel.clone(), interceptor.clone());
        let local = match mode {
            EvaluationMode::Remote => None,
            EvaluationMode::Local => Some(LocalEvaluator::bootstrap(evaluation.clone()).await?),
        };
        Ok(Self {
            evaluation,
            admin: AdminClient::with_interceptor(channel, interceptor),
            local,
        })
    }

    pub fn admin(&self) -> AdminClient<IdentifiedChannel> {
        self.admin.clone()
    }

    pub async fn resolve_bool(
        &self,
        flag_key: impl Into<String>,
        context: impl Into<EvaluationContext>,
    ) -> Result<Resolution<bool>, Error> {
        let (flag_key, context) = (flag_key.into(), context.into());
        if let Some(local) = &self.local {
            return Ok(local.resolve_bool(&flag_key, context));
        }
        let resp = self
            .evaluation
            .clone()
            .resolve_boolean(request(flag_key, context))
            .await?
            .into_inner();
        Ok(resolution(resp.value, resp.meta))
    }

    pub async fn resolve_string(
        &self,
        flag_key: impl Into<String>,
        context: impl Into<EvaluationContext>,
    ) -> Result<Resolution<String>, Error> {
        let (flag_key, context) = (flag_key.into(), context.into());
        if let Some(local) = &self.local {
            return Ok(local.resolve_string(&flag_key, context));
        }
        let resp = self
            .evaluation
            .clone()
            .resolve_string(request(flag_key, context))
            .await?
            .into_inner();
        Ok(resolution(resp.value, resp.meta))
    }

    pub async fn resolve_int(
        &self,
        flag_key: impl Into<String>,
        context: impl Into<EvaluationContext>,
    ) -> Result<Resolution<i64>, Error> {
        let (flag_key, context) = (flag_key.into(), context.into());
        if let Some(local) = &self.local {
            return Ok(local.resolve_int(&flag_key, context));
        }
        let resp = self
            .evaluation
            .clone()
            .resolve_integer(request(flag_key, context))
            .await?
            .into_inner();
        Ok(resolution(resp.value, resp.meta))
    }

    pub async fn resolve_float(
        &self,
        flag_key: impl Into<String>,
        context: impl Into<EvaluationContext>,
    ) -> Result<Resolution<f64>, Error> {
        let (flag_key, context) = (flag_key.into(), context.into());
        if let Some(local) = &self.local {
            return Ok(local.resolve_float(&flag_key, context));
        }
        let resp = self
            .evaluation
            .clone()
            .resolve_float(request(flag_key, context))
            .await?
            .into_inner();
        Ok(resolution(resp.value, resp.meta))
    }

    pub async fn resolve_object(
        &self,
        flag_key: impl Into<String>,
        context: impl Into<EvaluationContext>,
    ) -> Result<Resolution<prost_types::Struct>, Error> {
        let (flag_key, context) = (flag_key.into(), context.into());
        if let Some(local) = &self.local {
            return Ok(local.resolve_object(&flag_key, context));
        }
        let resp = self
            .evaluation
            .clone()
            .resolve_object(request(flag_key, context))
            .await?
            .into_inner();
        Ok(resolution(resp.value.unwrap_or_default(), resp.meta))
    }

    pub async fn resolve_all(
        &self,
        context: impl Into<EvaluationContext>,
    ) -> Result<ResolveAllResponse, Error> {
        let context = context.into();
        if let Some(local) = &self.local {
            return Ok(local.resolve_all(context));
        }
        Ok(self
            .evaluation
            .clone()
            .resolve_all(feature_flag_proto::ResolveAllRequest {
                context: Some(context),
            })
            .await?
            .into_inner())
    }

    pub async fn subscribe_events(&self) -> Result<Streaming<Event>, Error> {
        Ok(self
            .evaluation
            .clone()
            .stream_events(feature_flag_proto::EventStreamRequest {})
            .await?
            .into_inner())
    }
}

fn request(flag_key: String, context: EvaluationContext) -> ResolveRequest {
    ResolveRequest {
        flag_key,
        context: Some(context),
    }
}

fn resolution<T>(value: T, meta: Option<ResolutionMeta>) -> Resolution<T> {
    let meta = meta.unwrap_or_default();
    let error_code = (!meta.error_code.is_empty()).then_some(meta.error_code);
    Resolution {
        value,
        variant: meta.variant,
        reason: Reason::try_from(meta.reason).unwrap_or(Reason::Unspecified),
        error_code,
    }
}
