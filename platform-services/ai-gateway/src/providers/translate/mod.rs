//! Translation between the Anthropic Messages and OpenAI Chat Completions dialects,
//! delegated to the `llm-bridge-core` protocol bridge.
//!
//! The gateway exposes both `/v1/messages` (Anthropic) and `/v1/chat/completions`
//! (OpenAI); a model may live behind a provider of the other dialect. When the client
//! and provider dialects differ, requests, responses, and streams (via [`SseTranslator`])
//! are translated here. Matching dialects pass through untouched.

mod streaming;

pub use streaming::SseTranslator;

use std::collections::HashMap;

use bytes::Bytes;
use chrono::Utc;
use llm_bridge_core::model::{TransformError, TransformRequest, TransformResponse};
use llm_bridge_core::transform;
use serde_json::Value;

use super::Dialect;
use crate::error::{GatewayError, Result};

/// Anthropic's `/v1/messages` requires `max_tokens`, which OpenAI clients may omit and the
/// bridge leaves unset; backfill this default so translated requests stay valid.
const DEFAULT_ANTHROPIC_MAX_TOKENS: u64 = 4096;

/// Translate a request body from the `source` dialect into the `target` dialect.
pub fn translate_request(body: &[u8], source: Dialect, target: Dialect) -> Result<Bytes> {
    let result = match (source, target) {
        (Dialect::Anthropic, Dialect::OpenAiCompatible) => {
            transform::anthropic_to_openai(&request("/v1/messages", body))
        }
        (Dialect::OpenAiCompatible, Dialect::Anthropic) => {
            transform::openai_to_anthropic(&request("/v1/chat/completions", body))
        }
        _ => return Ok(Bytes::copy_from_slice(body)),
    };

    let bytes = body_of(result)?;
    if target == Dialect::Anthropic {
        return ensure_max_tokens_and_caching(bytes);
    }

    Ok(bytes)
}

fn ensure_max_tokens_and_caching(body: Bytes) -> Result<Bytes> {
    let mut json: Value =
        serde_json::from_slice(&body).map_err(|e| GatewayError::BadRequest(e.to_string()))?;

    if json.get("max_tokens").is_some() {
        return Ok(body);
    }

    json["max_tokens"] = Value::from(DEFAULT_ANTHROPIC_MAX_TOKENS);
    // openai does automatic prompt caching, anthropic does not
    json["cache_control"] = serde_json::json!({"type": "ephemeral"});

    serde_json::to_vec(&json)
        .map(Bytes::from)
        .map_err(|e| GatewayError::BadRequest(e.to_string()))
}

/// Translate a response body from the `source` (provider) dialect into the `target`
/// (client) dialect.
pub fn translate_response(body: &[u8], source: Dialect, target: Dialect) -> Result<Bytes> {
    let result = match (source, target) {
        (Dialect::OpenAiCompatible, Dialect::Anthropic) => {
            transform::openai_response_to_anthropic_message(&request("/v1/chat/completions", body))
        }
        (Dialect::Anthropic, Dialect::OpenAiCompatible) => {
            transform::anthropic_response_to_openai_response(&request("/v1/messages", body))
        }
        _ => return Ok(Bytes::copy_from_slice(body)),
    };
    let bytes = body_of(result)?;
    if target == Dialect::OpenAiCompatible {
        return ensure_created(bytes);
    }
    Ok(bytes)
}

/// The OpenAI Chat Completion object requires a `created` timestamp, which the bridge omits;
/// backfill it so responses stay schema-valid for OpenAI clients.
fn ensure_created(body: Bytes) -> Result<Bytes> {
    let mut json: Value =
        serde_json::from_slice(&body).map_err(|e| GatewayError::BadRequest(e.to_string()))?;
    if json.get("created").is_some() {
        return Ok(body);
    }
    json["created"] = Value::from(Utc::now().timestamp());
    serde_json::to_vec(&json)
        .map(Bytes::from)
        .map_err(|e| GatewayError::BadRequest(e.to_string()))
}

fn request(path: &str, body: &[u8]) -> TransformRequest {
    TransformRequest {
        headers: HashMap::new(),
        path: path.to_owned(),
        body: Bytes::copy_from_slice(body),
    }
}

fn body_of(result: std::result::Result<TransformResponse, TransformError>) -> Result<Bytes> {
    result
        .map(|r| r.body)
        .map_err(|e| GatewayError::BadRequest(e.sanitized_message()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn req_v(body: &Value, source: Dialect, target: Dialect) -> Value {
        let out = translate_request(&serde_json::to_vec(body).unwrap(), source, target).unwrap();
        serde_json::from_slice(&out).unwrap()
    }

    fn resp_v(body: &Value, target: Dialect, source: Dialect) -> Value {
        let out = translate_response(&serde_json::to_vec(body).unwrap(), source, target).unwrap();
        serde_json::from_slice(&out).unwrap()
    }

    #[test]
    fn anthropic_request_becomes_openai() {
        let req = json!({
            "model": "gpt-4o", "system": "be brief", "max_tokens": 100,
            "stop_sequences": ["X"], "temperature": 0.5,
            "messages": [{ "role": "user", "content": "hi" }],
        });
        let out = req_v(&req, Dialect::Anthropic, Dialect::OpenAiCompatible);
        assert_eq!(out["messages"][0]["role"], "system");
        assert_eq!(out["messages"][1]["content"], "hi");
        assert_eq!(out["stop"][0], "X");
        assert_eq!(out["temperature"], 0.5);
    }

    #[test]
    fn openai_request_becomes_anthropic() {
        let req = json!({
            "model": "claude-opus-4-8",
            "messages": [
                { "role": "system", "content": "be brief" },
                { "role": "user", "content": "hi" },
            ],
        });
        let out = req_v(&req, Dialect::OpenAiCompatible, Dialect::Anthropic);
        assert_eq!(out["system"], "be brief");
        assert_eq!(out["messages"].as_array().unwrap().len(), 1);
        assert_eq!(out["max_tokens"], 4096);
    }

    #[test]
    fn openai_response_becomes_anthropic() {
        let resp = json!({
            "id": "chatcmpl-1", "model": "gpt-4o",
            "choices": [{ "index": 0, "message": { "role": "assistant", "content": "hi there" },
                          "finish_reason": "stop" }],
            "usage": { "prompt_tokens": 9, "completion_tokens": 4 },
        });
        let out = resp_v(&resp, Dialect::Anthropic, Dialect::OpenAiCompatible);
        assert_eq!(out["content"][0]["text"], "hi there");
        assert_eq!(out["stop_reason"], "end_turn");
        assert_eq!(out["usage"]["input_tokens"], 9);
        assert_eq!(out["usage"]["output_tokens"], 4);
    }

    #[test]
    fn anthropic_response_becomes_openai() {
        let resp = json!({
            "id": "msg_1", "model": "claude-opus-4-8", "role": "assistant",
            "content": [{ "type": "text", "text": "hi there" }],
            "stop_reason": "max_tokens",
            "usage": { "input_tokens": 9, "output_tokens": 4 },
        });
        let out = resp_v(&resp, Dialect::OpenAiCompatible, Dialect::Anthropic);
        assert_eq!(out["choices"][0]["message"]["content"], "hi there");
        assert_eq!(out["choices"][0]["finish_reason"], "length");
    }

    #[test]
    fn anthropic_tool_definitions_become_openai() {
        let req = json!({
            "model": "gpt-4o", "max_tokens": 100,
            "messages": [{ "role": "user", "content": "weather?" }],
            "tools": [{ "name": "get_weather", "description": "look it up",
                        "input_schema": { "type": "object" } }],
            "tool_choice": { "type": "any" },
        });
        let out = req_v(&req, Dialect::Anthropic, Dialect::OpenAiCompatible);
        assert_eq!(out["tools"][0]["type"], "function");
        assert_eq!(out["tools"][0]["function"]["name"], "get_weather");
    }
}
