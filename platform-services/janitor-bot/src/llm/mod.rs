use anyhow::Context;
use async_openai::Client;
use async_openai::config::{Config, OpenAIConfig};
use async_openai::types::chat::{
    ChatCompletionRequestMessage, ChatCompletionResponseMessage, ChatCompletionTools,
    CreateChatCompletionRequestArgs,
};
use http::HeaderMap;
use opentelemetry::global;
use opentelemetry::propagation::Injector;
use secrecy::SecretString;
use tracing_opentelemetry::OpenTelemetrySpanExt;

const DEFAULT_GATEWAY_URL: &str = "http://api.ai-gateway.svc.cluster.local";

/// Adapts an [`http::HeaderMap`] to OpenTelemetry's [`Injector`] so the active
/// trace context can be written as W3C `traceparent`/`tracestate` headers.
struct HeaderInjector<'a>(&'a mut HeaderMap);

impl Injector for HeaderInjector<'_> {
    fn set(&mut self, key: &str, value: String) {
        if let (Ok(name), Ok(val)) = (
            key.parse::<http::HeaderName>(),
            http::HeaderValue::from_str(&value),
        ) {
            self.0.insert(name, val);
        }
    }
}

/// Wraps [`OpenAIConfig`] to inject the current span's trace context into the
/// request headers. async-openai calls `headers()` once per request, so each
/// downstream call to the AI gateway carries a fresh `traceparent`, letting the
/// gateway continue this trace.
#[derive(Clone)]
struct TraceContextConfig {
    inner: OpenAIConfig,
}

impl Config for TraceContextConfig {
    fn headers(&self) -> HeaderMap {
        let mut headers = self.inner.headers();
        let cx = tracing::Span::current().context();
        global::get_text_map_propagator(|propagator| {
            propagator.inject_context(&cx, &mut HeaderInjector(&mut headers));
        });
        headers
    }

    fn url(&self, path: &str) -> String {
        self.inner.url(path)
    }

    fn query(&self) -> Vec<(&str, &str)> {
        self.inner.query()
    }

    fn api_base(&self) -> &str {
        self.inner.api_base()
    }

    fn api_key(&self) -> &SecretString {
        self.inner.api_key()
    }
}

/// A thin wrapper around the OpenAI-compatible AI gateway. It holds no
/// domain knowledge — callers supply their own messages, tools, and model.
pub struct LlmClient {
    client: Client<TraceContextConfig>,
}

impl LlmClient {
    /// Constructs the client from the environment, or `None` when no
    /// `AI_GATEWAY_TOKEN` is configured (the feature degrades to disabled).
    pub fn from_env() -> Option<Self> {
        let token = std::env::var("AI_GATEWAY_TOKEN")
            .ok()
            .filter(|t| !t.is_empty())?;
        let base = std::env::var("AI_GATEWAY_URL")
            .ok()
            .filter(|u| !u.is_empty())
            .unwrap_or_else(|| DEFAULT_GATEWAY_URL.to_owned());

        Some(Self::new(base, token))
    }

    /// Builds a client pointed at `base_url` (without the trailing `/v1`).
    pub fn new(base_url: String, token: String) -> Self {
        let inner = OpenAIConfig::new()
            .with_api_base(format!("{}/v1", base_url.trim_end_matches('/')))
            .with_api_key(token);

        Self {
            client: Client::with_config(TraceContextConfig { inner }),
        }
    }

    /// Sends one chat-completion turn and returns the assistant message.
    ///
    /// The request is wrapped in a client span (`llm.*` attributes) whose
    /// trace context is propagated to the AI gateway via `traceparent`, so the
    /// gateway-side spans link back into this trace.
    #[tracing::instrument(
        name = "llm.chat",
        skip_all,
        fields(
            otel.name = format!("chat {model}"),
            otel.kind = "client",
            llm.system = "openai",
            llm.request.model = model,
            llm.request.message_count = messages.len(),
            llm.response.finish_reason = tracing::field::Empty,
            llm.response.tool_call_count = tracing::field::Empty,
            llm.usage.input_tokens = tracing::field::Empty,
            llm.usage.output_tokens = tracing::field::Empty,
        )
    )]
    pub async fn chat(
        &self,
        model: &str,
        messages: Vec<ChatCompletionRequestMessage>,
        tools: Vec<ChatCompletionTools>,
    ) -> anyhow::Result<ChatCompletionResponseMessage> {
        let request = CreateChatCompletionRequestArgs::default()
            .model(model)
            .tools(tools)
            .messages(messages)
            .build()?;

        let resp = self.client.chat().create(request).await?;

        let span = tracing::Span::current();
        if let Some(usage) = resp.usage.as_ref() {
            span.record("llm.usage.input_tokens", usage.prompt_tokens);
            span.record("llm.usage.output_tokens", usage.completion_tokens);
        }

        let choice = resp
            .choices
            .into_iter()
            .next()
            .context("LLM returned no choices")?;

        if let Some(reason) = choice.finish_reason {
            span.record("llm.response.finish_reason", format!("{reason:?}"));
        }
        span.record(
            "llm.response.tool_call_count",
            choice.message.tool_calls.as_ref().map_or(0, Vec::len),
        );

        Ok(choice.message)
    }
}
