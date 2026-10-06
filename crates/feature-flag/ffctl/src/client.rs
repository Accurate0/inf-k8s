use crate::auth::TokenSource;
use crate::convert::Convert;
use anyhow::{Context as _, bail};
use feature_flag_proto as pb;
use pb::admin_client::AdminClient;
use pb::evaluation_client::EvaluationClient;
use serde_json::{Map, Value as Json};
use std::time::Duration;
use tonic::metadata::{Ascii, MetadataValue};
use tonic::service::Interceptor;
use tonic::service::interceptor::InterceptedService;
use tonic::transport::{Channel, ClientTlsConfig, Endpoint};
use tonic::{Request, Response, Status, Streaming};

#[derive(Clone)]
pub enum Identity {
    Session(TokenSource),
    Token(String),
    Actor(String),
}

impl Identity {
    pub fn local_actor(explicit: Option<String>) -> Self {
        if let Some(actor) = explicit.filter(|actor| !actor.is_empty()) {
            return Self::Actor(actor);
        }

        let actor = std::process::Command::new("git")
            .args(["config", "user.email"])
            .output()
            .ok()
            .filter(|output| output.status.success())
            .and_then(|output| String::from_utf8(output.stdout).ok())
            .map(|email| email.trim().to_owned())
            .filter(|email| !email.is_empty())
            .unwrap_or_else(|| "ffctl".to_owned());

        Self::Actor(actor)
    }

    pub fn display_name(&self) -> String {
        match self {
            Self::Session(tokens) => tokens.username(),
            Self::Token(_) => "token".to_owned(),
            Self::Actor(actor) => format!("{actor} (no auth)"),
        }
    }

    async fn ensure_fresh(&self) -> anyhow::Result<()> {
        let Self::Session(tokens) = self else {
            return Ok(());
        };

        tokens.ensure_fresh().await
    }
}

#[derive(Clone)]
pub struct AuthInterceptor {
    identity: Identity,
}

impl AuthInterceptor {
    const CLIENT_ID: &'static str = "ffctl";

    fn header(value: &str) -> Result<MetadataValue<Ascii>, Status> {
        value
            .parse()
            .map_err(|_| Status::invalid_argument("identity is not a valid header value"))
    }
}

impl Interceptor for AuthInterceptor {
    fn call(&mut self, mut request: Request<()>) -> Result<Request<()>, Status> {
        let metadata = request.metadata_mut();
        metadata.insert("client-id", MetadataValue::from_static(Self::CLIENT_ID));

        match &self.identity {
            Identity::Session(tokens) => {
                let bearer = format!("Bearer {}", tokens.access_token());
                metadata.insert("authorization", Self::header(&bearer)?);
            }
            Identity::Token(token) => {
                metadata.insert("authorization", Self::header(&format!("Bearer {token}"))?);
            }
            Identity::Actor(actor) => {
                metadata.insert("actor", Self::header(actor)?);
            }
        }

        Ok(request)
    }
}

type Authed = InterceptedService<Channel, AuthInterceptor>;

#[derive(Debug, Clone, Default)]
pub struct EvalInput {
    pub targeting_key: String,
    pub attributes: Map<String, Json>,
}

impl EvalInput {
    fn context(&self) -> pb::EvaluationContext {
        pb::EvaluationContext {
            targeting_key: self.targeting_key.clone(),
            attributes: Some(Convert::json_to_struct(&self.attributes)),
        }
    }
}

#[derive(Clone)]
pub struct Api {
    url: String,
    identity: Identity,
    admin: AdminClient<Authed>,
    evaluation: EvaluationClient<Authed>,
}

impl Api {
    pub async fn connect(url: &str, identity: Identity) -> anyhow::Result<Self> {
        let mut endpoint = Endpoint::from_shared(url.to_owned())
            .with_context(|| format!("invalid url {url}"))?
            .connect_timeout(Duration::from_secs(10))
            .http2_keep_alive_interval(Duration::from_secs(20))
            .keep_alive_timeout(Duration::from_secs(10))
            .keep_alive_while_idle(true);

        if url.starts_with("https://") {
            endpoint = endpoint
                .tls_config(ClientTlsConfig::new().with_native_roots())
                .context("configuring tls")?;
        }

        let channel = endpoint
            .connect()
            .await
            .with_context(|| format!("connecting to {url}"))?;

        let interceptor = AuthInterceptor {
            identity: identity.clone(),
        };

        Ok(Self {
            url: url.to_owned(),
            identity,
            admin: AdminClient::with_interceptor(channel.clone(), interceptor.clone()),
            evaluation: EvaluationClient::with_interceptor(channel, interceptor),
        })
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn user(&self) -> String {
        self.identity.display_name()
    }

    pub async fn keep_fresh(&self) -> anyhow::Result<()> {
        self.identity.ensure_fresh().await
    }

    async fn admin(&self) -> anyhow::Result<AdminClient<Authed>> {
        self.identity.ensure_fresh().await?;

        Ok(self.admin.clone())
    }

    async fn evaluation(&self) -> anyhow::Result<EvaluationClient<Authed>> {
        self.identity.ensure_fresh().await?;

        Ok(self.evaluation.clone())
    }

    fn unwrap<T>(result: Result<Response<T>, Status>) -> anyhow::Result<T> {
        match result {
            Ok(response) => Ok(response.into_inner()),
            Err(status) if status.code() == tonic::Code::Unauthenticated => {
                bail!("unauthenticated: {} (run `ffctl login`)", status.message())
            }
            Err(status) => bail!("{:?}: {}", status.code(), status.message()),
        }
    }

    pub async fn list_flags(&self, include_archived: bool) -> anyhow::Result<Vec<pb::Flag>> {
        let request = pb::ListFlagsRequest { include_archived };
        let mut flags = Self::unwrap(self.admin().await?.list_flags(request).await)?.flags;
        flags.sort_by(|a, b| a.key.cmp(&b.key));

        Ok(flags)
    }

    pub async fn get_flag(&self, key: &str) -> anyhow::Result<pb::Flag> {
        let request = pb::GetFlagRequest {
            key: key.to_owned(),
        };

        Self::unwrap(self.admin().await?.get_flag(request).await)
    }

    pub async fn create_flag(&self, request: pb::CreateFlagRequest) -> anyhow::Result<pb::Flag> {
        Self::unwrap(self.admin().await?.create_flag(request).await)
    }

    pub async fn update_flag(
        &self,
        key: &str,
        enabled: bool,
        default_variant_key: &str,
    ) -> anyhow::Result<pb::Flag> {
        let request = pb::UpdateFlagRequest {
            key: key.to_owned(),
            enabled,
            default_variant_key: default_variant_key.to_owned(),
        };

        Self::unwrap(self.admin().await?.update_flag(request).await)
    }

    pub async fn archive_flag(&self, key: &str, archived: bool) -> anyhow::Result<pb::Flag> {
        let request = pb::ArchiveFlagRequest {
            key: key.to_owned(),
            archived,
        };

        Self::unwrap(self.admin().await?.archive_flag(request).await)
    }

    pub async fn delete_flag(&self, key: &str) -> anyhow::Result<()> {
        let request = pb::DeleteFlagRequest {
            key: key.to_owned(),
        };
        Self::unwrap(self.admin().await?.delete_flag(request).await)?;

        Ok(())
    }

    pub async fn upsert_variant(
        &self,
        flag_key: &str,
        variant: pb::Variant,
    ) -> anyhow::Result<pb::Flag> {
        let request = pb::UpsertVariantRequest {
            flag_key: flag_key.to_owned(),
            variant: Some(variant),
        };

        Self::unwrap(self.admin().await?.upsert_variant(request).await)
    }

    pub async fn delete_variant(
        &self,
        flag_key: &str,
        variant_key: &str,
    ) -> anyhow::Result<pb::Flag> {
        let request = pb::DeleteVariantRequest {
            flag_key: flag_key.to_owned(),
            variant_key: variant_key.to_owned(),
        };

        Self::unwrap(self.admin().await?.delete_variant(request).await)
    }

    pub async fn set_flag_rules(
        &self,
        flag_key: &str,
        rules: Vec<pb::Rule>,
    ) -> anyhow::Result<pb::Flag> {
        let request = pb::SetFlagRulesRequest {
            flag_key: flag_key.to_owned(),
            rules,
        };

        Self::unwrap(self.admin().await?.set_flag_rules(request).await)
    }

    pub async fn list_segments(&self) -> anyhow::Result<Vec<pb::Segment>> {
        let request = pb::ListSegmentsRequest {};
        let mut segments = Self::unwrap(self.admin().await?.list_segments(request).await)?.segments;
        segments.sort_by(|a, b| a.key.cmp(&b.key));

        Ok(segments)
    }

    pub async fn get_segment(&self, key: &str) -> anyhow::Result<pb::Segment> {
        let request = pb::GetSegmentRequest {
            key: key.to_owned(),
        };

        Self::unwrap(self.admin().await?.get_segment(request).await)
    }

    pub async fn upsert_segment(&self, segment: pb::Segment) -> anyhow::Result<pb::Segment> {
        let request = pb::UpdateSegmentRequest {
            segment: Some(segment),
        };

        Self::unwrap(self.admin().await?.update_segment(request).await)
    }

    pub async fn delete_segment(&self, key: &str) -> anyhow::Result<()> {
        let request = pb::DeleteSegmentRequest {
            key: key.to_owned(),
        };
        Self::unwrap(self.admin().await?.delete_segment(request).await)?;

        Ok(())
    }

    pub async fn apply_config(
        &self,
        request: pb::ApplyConfigRequest,
    ) -> anyhow::Result<pb::ApplyConfigResponse> {
        Self::unwrap(self.admin().await?.apply_config(request).await)
    }

    pub async fn list_changes(
        &self,
        target_kind: &str,
        target_key: &str,
        limit: u32,
    ) -> anyhow::Result<Vec<pb::FlagChange>> {
        let request = pb::ListChangesRequest {
            target_kind: target_kind.to_owned(),
            target_key: target_key.to_owned(),
            limit,
        };

        Ok(Self::unwrap(self.admin().await?.list_changes(request).await)?.changes)
    }

    pub async fn apply_flag(
        &self,
        current: Option<&pb::Flag>,
        desired: &pb::Flag,
    ) -> anyhow::Result<pb::Flag> {
        let Some(current) = current else {
            let created = self
                .create_flag(pb::CreateFlagRequest {
                    key: desired.key.clone(),
                    value_type: desired.value_type,
                    enabled: desired.enabled,
                    default_variant_key: desired.default_variant_key.clone(),
                    variants: desired.variants.clone(),
                })
                .await?;

            if desired.rules.is_empty() {
                return Ok(created);
            }

            return self
                .set_flag_rules(&desired.key, desired.rules.clone())
                .await;
        };

        if current.key != desired.key {
            bail!("a flag's key cannot be changed; create a new flag instead");
        }

        if current.value_type != desired.value_type {
            bail!("a flag's type cannot be changed; create a new flag instead");
        }

        for variant in &desired.variants {
            let unchanged = current.variants.iter().any(|existing| existing == variant);

            if !unchanged {
                self.upsert_variant(&desired.key, variant.clone()).await?;
            }
        }

        if Self::unranked(&current.rules) != Self::unranked(&desired.rules) {
            self.set_flag_rules(&desired.key, desired.rules.clone())
                .await?;
        }

        let settings_changed = current.enabled != desired.enabled
            || current.default_variant_key != desired.default_variant_key;

        if settings_changed {
            self.update_flag(&desired.key, desired.enabled, &desired.default_variant_key)
                .await?;
        }

        for variant in &current.variants {
            let kept = desired
                .variants
                .iter()
                .any(|wanted| wanted.key == variant.key);

            if !kept {
                self.delete_variant(&desired.key, &variant.key).await?;
            }
        }

        self.get_flag(&desired.key).await
    }

    fn unranked(rules: &[pb::Rule]) -> Vec<pb::Rule> {
        rules
            .iter()
            .cloned()
            .map(|mut rule| {
                rule.rank = 0;
                rule
            })
            .collect()
    }

    pub async fn resolve_all(&self, input: &EvalInput) -> anyhow::Result<Vec<pb::EvaluatedFlag>> {
        let request = pb::ResolveAllRequest {
            context: Some(input.context()),
        };
        let mut flags = Self::unwrap(self.evaluation().await?.resolve_all(request).await)?.flags;
        flags.sort_by(|a, b| a.flag_key.cmp(&b.flag_key));

        Ok(flags)
    }

    pub async fn resolve(
        &self,
        flag_key: &str,
        input: &EvalInput,
    ) -> anyhow::Result<pb::EvaluatedFlag> {
        use prost_types::value::Kind;

        let mut evaluation = self.evaluation().await?;

        let snapshot = Self::unwrap(evaluation.get_snapshot(pb::GetSnapshotRequest {}).await)?;
        let flag = snapshot
            .flags
            .into_iter()
            .find(|flag| flag.key == flag_key)
            .with_context(|| format!("flag {flag_key} not found"))?;

        let request = pb::ResolveRequest {
            flag_key: flag_key.to_owned(),
            context: Some(input.context()),
        };

        let (kind, meta) = match flag.value_type() {
            pb::ValueType::Boolean => {
                let resolved = Self::unwrap(evaluation.resolve_boolean(request).await)?;
                (Kind::BoolValue(resolved.value), resolved.meta)
            }
            pb::ValueType::String => {
                let resolved = Self::unwrap(evaluation.resolve_string(request).await)?;
                (Kind::StringValue(resolved.value), resolved.meta)
            }
            pb::ValueType::Integer => {
                let resolved = Self::unwrap(evaluation.resolve_integer(request).await)?;
                (Kind::NumberValue(resolved.value as f64), resolved.meta)
            }
            pb::ValueType::Float => {
                let resolved = Self::unwrap(evaluation.resolve_float(request).await)?;
                (Kind::NumberValue(resolved.value), resolved.meta)
            }
            pb::ValueType::Object => {
                let resolved = Self::unwrap(evaluation.resolve_object(request).await)?;
                (
                    Kind::StructValue(resolved.value.unwrap_or_default()),
                    resolved.meta,
                )
            }
            pb::ValueType::Unspecified => bail!("flag {flag_key} has no type"),
        };

        Ok(pb::EvaluatedFlag {
            flag_key: flag_key.to_owned(),
            value_type: flag.value_type,
            value: Some(prost_types::Value { kind: Some(kind) }),
            meta,
        })
    }

    pub async fn stream_snapshot(&self) -> anyhow::Result<Streaming<pb::SnapshotResponse>> {
        let request = pb::GetSnapshotRequest {};

        Self::unwrap(self.evaluation().await?.stream_snapshot(request).await)
    }
}
