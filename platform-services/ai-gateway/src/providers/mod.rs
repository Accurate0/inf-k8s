pub mod anthropic;
pub mod openai;
pub mod registry;
pub mod translate;

pub use anthropic::Anthropic;
pub use openai::OpenAiCompatible;
pub use registry::Registry;

use bytes::Bytes;
use llm_bridge_core::model::ApiFormat;
use reqwest::{Client, RequestBuilder};
use serde_json::Value;

use crate::error::{GatewayError, Result};

#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Dialect {
    Anthropic,
    #[serde(alias = "openai")]
    OpenAiCompatible,
}

impl Dialect {
    /// The wire dialect a proxied sub-path speaks to the client. Differs from the
    /// provider's dialect when the proxy must translate (see [`super::translate`]).
    pub fn for_sub_path(sub_path: &str) -> Self {
        if sub_path.ends_with("/messages") {
            Dialect::Anthropic
        } else {
            Dialect::OpenAiCompatible
        }
    }

    /// The `llm-bridge-core` protocol format this dialect maps to.
    pub fn api_format(self) -> ApiFormat {
        match self {
            Dialect::Anthropic => ApiFormat::AnthropicMessages,
            Dialect::OpenAiCompatible => ApiFormat::OpenaiChat,
        }
    }
}

/// What an endpoint expects from a model. Embedding models are only reachable from the
/// embeddings endpoint; chat/messages endpoints only route chat models.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ModelKind {
    Chat,
    Embedding,
}

impl ModelKind {
    /// The kind of model a proxied sub-path serves.
    pub fn for_sub_path(sub_path: &str) -> Self {
        if sub_path.ends_with("/embeddings") {
            ModelKind::Embedding
        } else {
            ModelKind::Chat
        }
    }
}

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Usage {
    pub input: i64,
    pub output: i64,
}

pub trait Provider: Send + Sync {
    fn name(&self) -> &str;
    fn dialect(&self) -> Dialect;

    /// Builds the authenticated upstream request against this provider's *native* path
    /// for the given model kind (e.g. an OpenAI provider always posts to
    /// `/chat/completions`, never the inbound `/v1/messages`). The caller drives
    /// `.send()`, keeping the trait object-safe.
    fn build_request(&self, http: &Client, kind: ModelKind, body: Bytes) -> RequestBuilder;

    fn parse_usage(&self, body: &[u8]) -> Usage;
    fn parse_stream_usage(&self, body: &[u8]) -> Usage;
}

/// A parsed chat/messages request body, with the helpers the proxy needs to route and
/// rewrite it.
pub struct ProxyRequest {
    json: Value,
}

impl ProxyRequest {
    pub fn from_slice(body: &[u8]) -> Result<Self> {
        let json =
            serde_json::from_slice(body).map_err(|e| GatewayError::BadRequest(e.to_string()))?;
        Ok(Self { json })
    }

    pub fn model(&self) -> Result<&str> {
        self.json
            .get("model")
            .and_then(Value::as_str)
            .ok_or_else(|| GatewayError::BadRequest("missing model".into()))
    }

    pub fn is_stream(&self) -> bool {
        self.json
            .get("stream")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }

    pub fn set_model(&mut self, model: &str) {
        self.json["model"] = Value::String(model.to_owned());
    }

    pub fn to_bytes(&self) -> Result<Bytes> {
        serde_json::to_vec(&self.json)
            .map(Bytes::from)
            .map_err(|e| GatewayError::BadRequest(e.to_string()))
    }
}

/// Shared SSE scanner: yields each non-empty JSON `data:` payload to `f`.
pub(crate) fn for_each_sse_event(body: &[u8], mut f: impl FnMut(&Value)) {
    for line in String::from_utf8_lossy(body).lines() {
        let Some(data) = line.strip_prefix("data:") else {
            continue;
        };
        let data = data.trim();
        if data.is_empty() || data == "[DONE]" {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<Value>(data) {
            f(&value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_request_reads_and_rewrites_model() {
        let mut req = ProxyRequest::from_slice(br#"{"model":"a","stream":true}"#).unwrap();
        assert_eq!(req.model().unwrap(), "a");
        assert!(req.is_stream());

        req.set_model("b");
        let rewritten = ProxyRequest::from_slice(&req.to_bytes().unwrap()).unwrap();
        assert_eq!(rewritten.model().unwrap(), "b");
    }

    #[test]
    fn chat_request_missing_model_is_error() {
        let req = ProxyRequest::from_slice(br#"{}"#).unwrap();
        assert!(req.model().is_err());
        assert!(!req.is_stream());
    }
}
