//! An [OpenFeature](https://openfeature.dev) provider (spec 0.3) backed by the
//! feature-flags gRPC service. Flag evaluation is delegated to the backend via
//! [`feature_flag_client`]; configuration-change events are surfaced both as the
//! provider's [`status`](FeatureFlagProvider::status) and as an observable
//! [`ProviderEvent`] stream.

mod convert;

pub use feature_flag_client::{self, EvaluationMode, FeatureFlagClient};

use async_trait::async_trait;
use convert::{context_to_client, error_from_code, reason_to_open_feature, struct_to_open_feature};
use feature_flag_client::{Error as ClientError, Resolution};
use feature_flag_proto::EventType;
use open_feature::provider::{FeatureProvider, ProviderMetadata, ProviderStatus, ResolutionDetails};
use open_feature::{
    EvaluationContext, EvaluationError, EvaluationErrorCode, EvaluationResult, StructValue,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Duration;
use tokio::sync::broadcast;

const STATUS_NOT_READY: u8 = 0;
const STATUS_READY: u8 = 1;
const STATUS_ERROR: u8 = 2;
const STATUS_STALE: u8 = 3;

/// Observable provider lifecycle events, fed by the backend's `StreamEvents` RPC.
#[derive(Clone, Debug)]
pub enum ProviderEvent {
    Ready,
    ConfigurationChanged { version: i64 },
    Stale,
    Error,
}

pub struct FeatureFlagProvider {
    client: FeatureFlagClient,
    metadata: ProviderMetadata,
    status: Arc<AtomicU8>,
    events: broadcast::Sender<ProviderEvent>,
}

impl FeatureFlagProvider {
    pub async fn connect(endpoint: impl Into<String>) -> Result<Self, ClientError> {
        Ok(Self::new(FeatureFlagClient::connect(endpoint).await?))
    }

    /// Connect with an explicit evaluation mode (remote RPC vs in-process local
    /// evaluation against the streamed snapshot).
    pub async fn connect_with(
        endpoint: impl Into<String>,
        mode: EvaluationMode,
    ) -> Result<Self, ClientError> {
        Ok(Self::new(
            FeatureFlagClient::connect_with(endpoint, mode).await?,
        ))
    }

    pub fn new(client: FeatureFlagClient) -> Self {
        let status = Arc::new(AtomicU8::new(STATUS_NOT_READY));
        let (events, _) = broadcast::channel(64);
        tokio::spawn(event_loop(client.clone(), status.clone(), events.clone()));
        Self {
            client,
            metadata: ProviderMetadata::new("feature-flags"),
            status,
            events,
        }
    }

    /// Subscribe to provider lifecycle events (ready/configuration-changed/stale/error).
    pub fn events(&self) -> broadcast::Receiver<ProviderEvent> {
        self.events.subscribe()
    }
}

async fn event_loop(
    client: FeatureFlagClient,
    status: Arc<AtomicU8>,
    events: broadcast::Sender<ProviderEvent>,
) {
    loop {
        match client.subscribe_events().await {
            Ok(mut stream) => loop {
                match stream.message().await {
                    Ok(Some(event)) => match EventType::try_from(event.r#type) {
                        Ok(EventType::Ready) => {
                            status.store(STATUS_READY, Ordering::Relaxed);
                            let _ = events.send(ProviderEvent::Ready);
                        }
                        Ok(EventType::ConfigurationChanged) => {
                            let _ = events.send(ProviderEvent::ConfigurationChanged {
                                version: event.config_version,
                            });
                        }
                        _ => {}
                    },
                    Ok(None) => break,
                    Err(e) => {
                        tracing::warn!("event stream error: {e}");
                        break;
                    }
                }
            },
            Err(e) => {
                tracing::warn!("failed to open event stream: {e}");
                status.store(STATUS_ERROR, Ordering::Relaxed);
                let _ = events.send(ProviderEvent::Error);
            }
        }
        status.store(STATUS_STALE, Ordering::Relaxed);
        let _ = events.send(ProviderEvent::Stale);
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

fn into_details<T>(
    result: Result<Resolution<T>, ClientError>,
) -> EvaluationResult<ResolutionDetails<T>> {
    let resolution = result.map_err(client_error)?;
    if let Some(code) = resolution.error_code {
        return Err(error_from_code(&code, None));
    }
    Ok(ResolutionDetails {
        value: resolution.value,
        variant: (!resolution.variant.is_empty()).then_some(resolution.variant),
        reason: Some(reason_to_open_feature(resolution.reason)),
        flag_metadata: None,
    })
}

fn client_error(e: ClientError) -> EvaluationError {
    EvaluationError {
        code: EvaluationErrorCode::General(e.to_string()),
        message: Some(e.to_string()),
    }
}

#[async_trait]
impl FeatureProvider for FeatureFlagProvider {
    fn metadata(&self) -> &ProviderMetadata {
        &self.metadata
    }

    fn status(&self) -> ProviderStatus {
        match self.status.load(Ordering::Relaxed) {
            STATUS_READY => ProviderStatus::Ready,
            STATUS_ERROR => ProviderStatus::Error,
            STATUS_STALE => ProviderStatus::STALE,
            _ => ProviderStatus::NotReady,
        }
    }

    async fn resolve_bool_value(
        &self,
        flag_key: &str,
        ctx: &EvaluationContext,
    ) -> EvaluationResult<ResolutionDetails<bool>> {
        into_details(self.client.resolve_bool(flag_key, context_to_client(ctx)).await)
    }

    async fn resolve_int_value(
        &self,
        flag_key: &str,
        ctx: &EvaluationContext,
    ) -> EvaluationResult<ResolutionDetails<i64>> {
        into_details(self.client.resolve_int(flag_key, context_to_client(ctx)).await)
    }

    async fn resolve_float_value(
        &self,
        flag_key: &str,
        ctx: &EvaluationContext,
    ) -> EvaluationResult<ResolutionDetails<f64>> {
        into_details(self.client.resolve_float(flag_key, context_to_client(ctx)).await)
    }

    async fn resolve_string_value(
        &self,
        flag_key: &str,
        ctx: &EvaluationContext,
    ) -> EvaluationResult<ResolutionDetails<String>> {
        into_details(self.client.resolve_string(flag_key, context_to_client(ctx)).await)
    }

    async fn resolve_struct_value(
        &self,
        flag_key: &str,
        ctx: &EvaluationContext,
    ) -> EvaluationResult<ResolutionDetails<StructValue>> {
        let result = self
            .client
            .resolve_object(flag_key, context_to_client(ctx))
            .await
            .map(|r| Resolution {
                value: struct_to_open_feature(r.value),
                variant: r.variant,
                reason: r.reason,
                error_code: r.error_code,
            });
        into_details(result)
    }
}
