use bytes::Bytes;
use reqwest::{Client, RequestBuilder};
use serde_json::Value;

use super::{Dialect, Provider, Usage, for_each_sse_event};

pub struct Anthropic {
    name: String,
    base_url: String,
    api_key: String,
    version: String,
}

impl Anthropic {
    pub fn new(name: impl Into<String>, base_url: impl Into<String>, api_key: String) -> Self {
        Self {
            name: name.into(),
            base_url: base_url.into(),
            api_key,
            version: std::env::var("ANTHROPIC_VERSION").unwrap_or_else(|_| "2023-06-01".into()),
        }
    }
}

impl Provider for Anthropic {
    fn name(&self) -> &str {
        &self.name
    }

    fn dialect(&self) -> Dialect {
        Dialect::Anthropic
    }

    fn build_request(&self, http: &Client, sub_path: &str, body: Bytes) -> RequestBuilder {
        http.post(format!("{}{sub_path}", self.base_url))
            .header("content-type", "application/json")
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", &self.version)
            .body(body)
    }

    fn parse_usage(&self, body: &[u8]) -> Usage {
        serde_json::from_slice::<Value>(body)
            .ok()
            .map(|v| usage_of(v.get("usage")))
            .unwrap_or_default()
    }

    fn parse_stream_usage(&self, body: &[u8]) -> Usage {
        // Input arrives on message_start, output on the terminal message_delta.
        let mut usage = Usage::default();
        for_each_sse_event(body, |event| {
            if let Some(u) = event.get("message").and_then(|m| m.get("usage")) {
                let started = usage_of(Some(u));
                if started.input > 0 {
                    usage.input = started.input;
                }
            }
            if let Some(u) = event.get("usage") {
                let delta = usage_of(Some(u));
                if delta.output > 0 {
                    usage.output = delta.output;
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
        input: u.get("input_tokens").and_then(Value::as_i64).unwrap_or(0),
        output: u.get("output_tokens").and_then(Value::as_i64).unwrap_or(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider() -> Anthropic {
        Anthropic::new("anthropic", "https://example.test", "key".into())
    }

    #[test]
    fn parses_buffered_usage() {
        let body = br#"{"usage":{"input_tokens":12,"output_tokens":7}}"#;
        assert_eq!(
            provider().parse_usage(body),
            Usage {
                input: 12,
                output: 7
            }
        );
    }

    #[test]
    fn parses_streamed_usage() {
        let sse = "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":40,\"output_tokens\":1}}}\n\
                   data: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":99}}\n\
                   data: [DONE]\n";
        assert_eq!(
            provider().parse_stream_usage(sse.as_bytes()),
            Usage {
                input: 40,
                output: 99
            }
        );
    }
}
