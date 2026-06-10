use bytes::Bytes;
use reqwest::{Client, RequestBuilder};
use serde_json::Value;

use super::{Dialect, Provider, Usage, for_each_sse_event};

/// OpenAI-compatible upstream: OpenAI, OpenRouter, Gemini's compat endpoint, etc.
pub struct OpenAiCompatible {
    name: String,
    base_url: String,
    api_key: String,
}

impl OpenAiCompatible {
    pub fn new(name: impl Into<String>, base_url: impl Into<String>, api_key: String) -> Self {
        Self {
            name: name.into(),
            base_url: base_url.into(),
            api_key,
        }
    }
}

impl Provider for OpenAiCompatible {
    fn name(&self) -> &str {
        &self.name
    }

    fn dialect(&self) -> Dialect {
        Dialect::OpenAiCompatible
    }

    fn build_request(&self, http: &Client, sub_path: &str, body: Bytes) -> RequestBuilder {
        http.post(format!("{}{sub_path}", self.base_url))
            .header("content-type", "application/json")
            .bearer_auth(&self.api_key)
            .body(body)
    }

    fn parse_usage(&self, body: &[u8]) -> Usage {
        serde_json::from_slice::<Value>(body)
            .ok()
            .map(|v| usage_of(v.get("usage")))
            .unwrap_or_default()
    }

    fn parse_stream_usage(&self, body: &[u8]) -> Usage {
        // The final chunk carries usage when the caller sets stream_options.
        let mut usage = Usage::default();
        for_each_sse_event(body, |event| {
            if let Some(u) = event.get("usage").filter(|u| !u.is_null()) {
                let found = usage_of(Some(u));
                if found.input > 0 {
                    usage.input = found.input;
                }
                if found.output > 0 {
                    usage.output = found.output;
                }
            }
        });
        usage
    }
}

fn usage_of(usage: Option<&Value>) -> Usage {
    let Some(u) = usage else {
        return Usage::default();
    };
    Usage {
        input: u.get("prompt_tokens").and_then(Value::as_i64).unwrap_or(0),
        output: u.get("completion_tokens").and_then(Value::as_i64).unwrap_or(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider() -> OpenAiCompatible {
        OpenAiCompatible::new("openai", "https://example.test/v1", "key".into())
    }

    #[test]
    fn parses_buffered_usage() {
        let body = br#"{"usage":{"prompt_tokens":30,"completion_tokens":5}}"#;
        assert_eq!(provider().parse_usage(body), Usage { input: 30, output: 5 });
    }

    #[test]
    fn parses_streamed_usage() {
        let sse = "data: {\"choices\":[]}\n\
                   data: {\"usage\":{\"prompt_tokens\":11,\"completion_tokens\":22}}\n\
                   data: [DONE]\n";
        assert_eq!(provider().parse_stream_usage(sse.as_bytes()), Usage { input: 11, output: 22 });
    }
}
