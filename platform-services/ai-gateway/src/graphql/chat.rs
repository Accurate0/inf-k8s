use async_graphql::{Context, Enum, Error, InputObject, Result, SimpleObject};
use axum::body::Bytes;
use axum::http::HeaderMap;
use futures::{Stream, StreamExt};
use serde_json::{Value, json};

use super::{Gql, state, user_key};
use crate::routes::proxy::proxy_as;

const CHAT_PATH: &str = "/chat/completions";
const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

#[derive(Enum, Copy, Clone, Eq, PartialEq)]
#[graphql(name = "AiGatewayChatRole")]
pub enum Role {
    System,
    User,
    Assistant,
}

impl Role {
    fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::User => "user",
            Self::Assistant => "assistant",
        }
    }
}

#[derive(InputObject)]
#[graphql(name = "AiGatewayChatMessageInput")]
pub struct MessageInput {
    pub role: Role,
    pub content: String,
}

#[derive(InputObject)]
#[graphql(name = "AiGatewayChatInput")]
pub struct ChatInput {
    pub model: String,
    pub messages: Vec<MessageInput>,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f64>,
}

impl ChatInput {
    fn body(&self, stream: bool) -> Bytes {
        let mut body = json!({
            "model": self.model,
            "messages": self.messages.iter().map(|m| json!({
                "role": m.role.as_str(),
                "content": m.content,
            })).collect::<Vec<_>>(),
            "stream": stream,
        });
        if let Some(max_tokens) = self.max_tokens {
            body["max_tokens"] = json!(max_tokens);
        }
        if let Some(temperature) = self.temperature {
            body["temperature"] = json!(temperature);
        }
        if stream {
            body["stream_options"] = json!({ "include_usage": true });
        }
        Bytes::from(body.to_string())
    }
}

#[derive(SimpleObject)]
#[graphql(name = "AiGatewayChatResult")]
pub struct ChatResult {
    pub model: String,
    pub content: String,
    pub finish_reason: Option<String>,
    pub input_tokens: i64,
    pub output_tokens: i64,
}

#[derive(SimpleObject, Default)]
#[graphql(name = "AiGatewayChatChunk")]
pub struct ChatChunk {
    pub delta: String,
    pub finish_reason: Option<String>,
    /// Set on the final chunk.
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
}

fn upstream_error(status: axum::http::StatusCode, body: &[u8]) -> Error {
    let message = serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|v| {
            v.pointer("/error/message")
                .or_else(|| v.get("error"))
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_else(|| String::from_utf8_lossy(body).into_owned());
    Error::new(format!("{status}: {message}"))
}

pub async fn complete(ctx: &Context<'_>, input: ChatInput) -> Result<ChatResult> {
    let key = user_key(ctx).await?;
    let response = proxy_as(
        state(ctx).clone(),
        key,
        HeaderMap::new(),
        input.body(false),
        CHAT_PATH,
    )
    .await
    .gql()?;

    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), MAX_RESPONSE_BYTES).await?;
    if !status.is_success() {
        return Err(upstream_error(status, &body));
    }

    let value: Value = serde_json::from_slice(&body)?;
    let choice = &value["choices"][0];
    Ok(ChatResult {
        model: value["model"].as_str().unwrap_or(&input.model).to_owned(),
        content: choice["message"]["content"]
            .as_str()
            .unwrap_or_default()
            .to_owned(),
        finish_reason: choice["finish_reason"].as_str().map(str::to_owned),
        input_tokens: value["usage"]["prompt_tokens"].as_i64().unwrap_or_default(),
        output_tokens: value["usage"]["completion_tokens"]
            .as_i64()
            .unwrap_or_default(),
    })
}

pub async fn stream(
    ctx: &Context<'_>,
    input: ChatInput,
) -> Result<impl Stream<Item = Result<ChatChunk>> + use<>> {
    let key = user_key(ctx).await?;
    let response = proxy_as(
        state(ctx).clone(),
        key,
        HeaderMap::new(),
        input.body(true),
        CHAT_PATH,
    )
    .await
    .gql()?;

    let status = response.status();
    if !status.is_success() {
        let body = axum::body::to_bytes(response.into_body(), MAX_RESPONSE_BYTES).await?;
        return Err(upstream_error(status, &body));
    }

    let mut body = response.into_body().into_data_stream();
    Ok(async_stream::try_stream! {
        let mut buffer = String::new();
        while let Some(bytes) = body.next().await {
            buffer.push_str(&String::from_utf8_lossy(&bytes.map_err(|e| Error::new(e.to_string()))?));
            while let Some(end) = buffer.find('\n') {
                let line = buffer[..end].trim().to_owned();
                buffer.drain(..=end);
                match parse_line(&line) {
                    Line::Chunk(chunk) => yield chunk,
                    Line::Done => return,
                    Line::Skip => {}
                }
            }
        }
    })
}

enum Line {
    Chunk(ChatChunk),
    Done,
    Skip,
}

fn parse_line(line: &str) -> Line {
    let Some(data) = line.strip_prefix("data:").map(str::trim) else {
        return Line::Skip;
    };
    if data == "[DONE]" {
        return Line::Done;
    }
    let Ok(value) = serde_json::from_str::<Value>(data) else {
        return Line::Skip;
    };

    let choice = &value["choices"][0];
    let chunk = ChatChunk {
        delta: choice["delta"]["content"]
            .as_str()
            .unwrap_or_default()
            .to_owned(),
        finish_reason: choice["finish_reason"].as_str().map(str::to_owned),
        input_tokens: value["usage"]["prompt_tokens"].as_i64(),
        output_tokens: value["usage"]["completion_tokens"].as_i64(),
    };
    let empty =
        chunk.delta.is_empty() && chunk.finish_reason.is_none() && chunk.input_tokens.is_none();
    if empty {
        Line::Skip
    } else {
        Line::Chunk(chunk)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sse_lines() {
        let Line::Chunk(chunk) =
            parse_line(r#"data: {"choices":[{"delta":{"content":"hi"},"finish_reason":null}]}"#)
        else {
            panic!("expected a chunk");
        };
        assert_eq!(chunk.delta, "hi");

        let Line::Chunk(last) =
            parse_line(r#"data: {"choices":[],"usage":{"prompt_tokens":3,"completion_tokens":5}}"#)
        else {
            panic!("expected usage");
        };
        assert_eq!(last.output_tokens, Some(5));

        assert!(matches!(parse_line("data: [DONE]"), Line::Done));
        assert!(matches!(parse_line(": keep-alive"), Line::Skip));
        assert!(matches!(
            parse_line(r#"data: {"choices":[{"delta":{"role":"assistant"}}]}"#),
            Line::Skip
        ));
    }
}
