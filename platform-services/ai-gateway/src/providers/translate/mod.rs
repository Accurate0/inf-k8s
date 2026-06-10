//! Translation between the Anthropic Messages and OpenAI Chat Completions dialects.
//!
//! The gateway exposes both `/v1/messages` (Anthropic) and `/v1/chat/completions`
//! (OpenAI); a model may live behind a provider of the other dialect. When the client
//! and provider dialects differ, requests and responses are translated here, including
//! streaming via [`SseTranslator`]. Text content and the common sampling/stop parameters
//! are translated; tool calls and non-text blocks are not, and are dropped.

mod streaming;
mod types;

pub use streaming::SseTranslator;

use serde::Deserialize;
use serde_json::Value;

use super::Dialect;
use types::*;

pub fn translate_request(body: &Value, from: Dialect, to: Dialect) -> Value {
    match (from, to) {
        (Dialect::Anthropic, Dialect::OpenAiCompatible) => {
            to_value(OpenAiChatRequest::from(parse::<AnthropicRequest>(body)))
        }
        (Dialect::OpenAiCompatible, Dialect::Anthropic) => {
            to_value(AnthropicMessagesRequest::from(parse::<OpenAiRequest>(body)))
        }
        _ => body.clone(),
    }
}

pub fn translate_response(body: &Value, from: Dialect, to: Dialect) -> Value {
    match (from, to) {
        (Dialect::Anthropic, Dialect::OpenAiCompatible) => {
            to_value(AnthropicMessageResponse::from(parse::<OpenAiResponse>(body)))
        }
        (Dialect::OpenAiCompatible, Dialect::Anthropic) => {
            to_value(OpenAiChatResponse::from(parse::<AnthropicResponse>(body)))
        }
        _ => body.clone(),
    }
}

fn parse<T: for<'de> Deserialize<'de> + Default>(body: &Value) -> T {
    serde_json::from_value(body.clone()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn anthropic_request_becomes_openai() {
        let req = json!({
            "model": "gpt-4o", "system": "be brief", "max_tokens": 100,
            "stop_sequences": ["X"], "temperature": 0.5,
            "messages": [{ "role": "user", "content": "hi" }],
        });
        let out = translate_request(&req, Dialect::Anthropic, Dialect::OpenAiCompatible);
        assert_eq!(out["messages"][0]["role"], "system");
        assert_eq!(out["messages"][0]["content"], "be brief");
        assert_eq!(out["messages"][1]["content"], "hi");
        assert_eq!(out["max_tokens"], 100);
        assert_eq!(out["stop"][0], "X");
        assert_eq!(out["temperature"], 0.5);
    }

    #[test]
    fn openai_request_becomes_anthropic_with_default_max_tokens() {
        let req = json!({
            "model": "claude-opus-4-8",
            "messages": [
                { "role": "system", "content": "be brief" },
                { "role": "user", "content": "hi" },
            ],
        });
        let out = translate_request(&req, Dialect::OpenAiCompatible, Dialect::Anthropic);
        assert_eq!(out["system"], "be brief");
        assert_eq!(out["messages"].as_array().unwrap().len(), 1);
        assert_eq!(out["max_tokens"], 4096);
    }

    #[test]
    fn openai_response_becomes_anthropic() {
        let resp = json!({
            "id": "chatcmpl-1", "model": "gpt-4o",
            "choices": [{ "message": { "role": "assistant", "content": "hi there" },
                          "finish_reason": "stop" }],
            "usage": { "prompt_tokens": 9, "completion_tokens": 4 },
        });
        let out = translate_response(&resp, Dialect::Anthropic, Dialect::OpenAiCompatible);
        assert_eq!(out["content"][0]["text"], "hi there");
        assert_eq!(out["stop_reason"], "end_turn");
        assert_eq!(out["usage"]["input_tokens"], 9);
        assert_eq!(out["usage"]["output_tokens"], 4);
    }

    #[test]
    fn anthropic_response_becomes_openai() {
        let resp = json!({
            "id": "msg_1", "model": "claude-opus-4-8",
            "content": [{ "type": "text", "text": "hi there" }],
            "stop_reason": "max_tokens",
            "usage": { "input_tokens": 9, "output_tokens": 4 },
        });
        let out = translate_response(&resp, Dialect::OpenAiCompatible, Dialect::Anthropic);
        assert_eq!(out["choices"][0]["message"]["content"], "hi there");
        assert_eq!(out["choices"][0]["finish_reason"], "length");
        assert_eq!(out["usage"]["total_tokens"], 13);
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
        let out = translate_request(&req, Dialect::Anthropic, Dialect::OpenAiCompatible);
        assert_eq!(out["tools"][0]["type"], "function");
        assert_eq!(out["tools"][0]["function"]["name"], "get_weather");
        assert_eq!(out["tools"][0]["function"]["parameters"]["type"], "object");
        assert_eq!(out["tool_choice"], "required");
    }

    #[test]
    fn openai_tool_call_history_becomes_anthropic_blocks() {
        let req = json!({
            "model": "claude-opus-4-8",
            "messages": [
                { "role": "user", "content": "weather?" },
                { "role": "assistant", "content": null,
                  "tool_calls": [{ "id": "call_1", "type": "function",
                    "function": { "name": "get_weather", "arguments": "{\"city\":\"SF\"}" } }] },
                { "role": "tool", "tool_call_id": "call_1", "content": "sunny" },
            ],
        });
        let out = translate_request(&req, Dialect::OpenAiCompatible, Dialect::Anthropic);

        let assistant = &out["messages"][1];
        assert_eq!(assistant["role"], "assistant");
        assert_eq!(assistant["content"][0]["type"], "tool_use");
        assert_eq!(assistant["content"][0]["id"], "call_1");
        assert_eq!(assistant["content"][0]["input"]["city"], "SF");

        let tool = &out["messages"][2];
        assert_eq!(tool["role"], "user");
        assert_eq!(tool["content"][0]["type"], "tool_result");
        assert_eq!(tool["content"][0]["tool_use_id"], "call_1");
        assert_eq!(tool["content"][0]["content"], "sunny");
    }

    #[test]
    fn openai_response_tool_call_becomes_anthropic_tool_use() {
        let resp = json!({
            "id": "chatcmpl-1", "model": "gpt-4o",
            "choices": [{ "message": { "role": "assistant", "content": null,
                "tool_calls": [{ "id": "call_1", "type": "function",
                    "function": { "name": "get_weather", "arguments": "{\"city\":\"SF\"}" } }] },
                "finish_reason": "tool_calls" }],
            "usage": { "prompt_tokens": 9, "completion_tokens": 4 },
        });
        let out = translate_response(&resp, Dialect::Anthropic, Dialect::OpenAiCompatible);
        assert_eq!(out["stop_reason"], "tool_use");
        assert_eq!(out["content"][0]["type"], "tool_use");
        assert_eq!(out["content"][0]["name"], "get_weather");
        assert_eq!(out["content"][0]["input"]["city"], "SF");
    }
}
