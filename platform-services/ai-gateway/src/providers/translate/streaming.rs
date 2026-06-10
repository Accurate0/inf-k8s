use serde_json::Value;

use super::Dialect;
use super::types::*;

/// Incrementally reshapes an upstream SSE byte stream from one dialect into the other.
/// [`push`](Self::push) returns translated SSE bytes to forward; [`finish`](Self::finish)
/// flushes closing frames. A matching-dialect translator passes bytes through unchanged.
pub struct SseTranslator {
    from: Dialect,
    to: Dialect,
    model: String,
    pending: String,
    started: bool,
    id: String,
    stop_reason: Option<String>,
    usage: AnthropicUsage,
    closed: bool,
}

impl SseTranslator {
    pub fn new(from: Dialect, to: Dialect, model: &str) -> Self {
        Self {
            from,
            to,
            model: model.to_owned(),
            pending: String::new(),
            started: false,
            id: String::new(),
            stop_reason: None,
            usage: AnthropicUsage::default(),
            closed: false,
        }
    }

    pub fn is_passthrough(&self) -> bool {
        self.from == self.to
    }

    pub fn push(&mut self, chunk: &[u8]) -> Vec<u8> {
        if self.is_passthrough() {
            return chunk.to_vec();
        }

        self.pending.push_str(&String::from_utf8_lossy(chunk));

        let mut out = String::new();
        while let Some(idx) = self.pending.find("\n\n") {
            let event: String = self.pending.drain(..idx + 2).collect();
            self.handle_event(&event, &mut out);
        }

        out.into_bytes()
    }

    pub fn finish(&mut self) -> Vec<u8> {
        if self.is_passthrough() {
            return Vec::new();
        }

        let mut out = String::new();
        if !self.pending.trim().is_empty() {
            let event = std::mem::take(&mut self.pending);
            self.handle_event(&event, &mut out);
        }

        self.close(&mut out);
        out.into_bytes()
    }

    fn handle_event(&mut self, event: &str, out: &mut String) {
        let mut data = String::new();
        for line in event.lines() {
            if let Some(rest) = line.strip_prefix("data:") {
                data.push_str(rest.trim());
            }
        }

        if data.is_empty() {
            return;
        }
        if data == "[DONE]" {
            self.close(out);
            return;
        }

        if let Ok(err) = serde_json::from_str::<StreamError>(&data) {
            self.emit_error(err.error, out);
            return;
        }

        match self.from {
            Dialect::OpenAiCompatible => {
                if let Ok(chunk) = serde_json::from_str::<OpenAiStreamChunk>(&data) {
                    self.on_openai_chunk(chunk, out);
                }
            }
            Dialect::Anthropic => {
                if let Ok(event) = serde_json::from_str::<AnthropicStreamEvent>(&data) {
                    self.on_anthropic_event(event, out);
                }
            }
        }
    }

    fn on_openai_chunk(&mut self, chunk: OpenAiStreamChunk, out: &mut String) {
        if self.id.is_empty()
            && let Some(id) = chunk.id
        {
            self.id = id;
        }

        if let Some(u) = chunk.usage {
            self.usage = AnthropicUsage { input_tokens: u.prompt_tokens, output_tokens: u.completion_tokens };
        }

        let Some(choice) = chunk.choices.into_iter().next() else {
            return;
        };

        if !self.started {
            self.started = true;

            emit_tagged(out, &AnthropicStreamOut::MessageStart {
                message: OutStartMessage {
                    id: self.id.clone(),
                    kind: "message",
                    role: "assistant",
                    model: self.model.clone(),
                    content: vec![],
                    stop_reason: None,
                    usage: AnthropicUsage::default(),
                },
            });
            emit_tagged(out, &AnthropicStreamOut::ContentBlockStart {
                index: 0,
                content_block: OutBlock::Text { text: String::new() },
            });
        }

        if let Some(text) = choice.delta.content.filter(|t| !t.is_empty()) {
            emit_tagged(out, &AnthropicStreamOut::ContentBlockDelta {
                index: 0,
                delta: TextDeltaOut { kind: "text_delta", text },
            });
        }

        if let Some(finish) = choice.finish_reason {
            self.stop_reason = Some(openai_finish_to_anthropic(Some(&finish)).into());
        }
    }

    fn on_anthropic_event(&mut self, event: AnthropicStreamEvent, out: &mut String) {
        match event {
            AnthropicStreamEvent::MessageStart { message } => {
                if let Some(id) = message.id {
                    self.id = id;
                }
                if let Some(u) = message.usage {
                    self.usage.input_tokens = u.input_tokens;
                }

                self.started = true;
                self.emit_openai(out, OpenAiDeltaOut { role: Some("assistant".into()), content: None }, None, None);
            }
            AnthropicStreamEvent::ContentBlockDelta { delta } => {
                if let Some(text) = delta.text {
                    self.emit_openai(out, OpenAiDeltaOut { role: None, content: Some(text) }, None, None);
                }
            }
            AnthropicStreamEvent::MessageDelta { delta, usage } => {
                if let Some(stop) = delta.stop_reason {
                    self.stop_reason = Some(anthropic_stop_to_openai(Some(&stop)).into());
                }
                if let Some(u) = usage {
                    self.usage.output_tokens = u.output_tokens;
                }
            }
            AnthropicStreamEvent::MessageStop => self.close(out),
            AnthropicStreamEvent::Other => {}
        }
    }

    fn emit_openai(
        &self,
        out: &mut String,
        delta: OpenAiDeltaOut,
        finish_reason: Option<String>,
        usage: Option<OpenAiUsage>,
    ) {
        let choices = if usage.is_some() {
            vec![]
        } else {
            vec![OpenAiStreamOutChoice { index: 0, delta, finish_reason }]
        };

        let frame = OpenAiStreamOut {
            id: self.id.clone(),
            object: "chat.completion.chunk",
            model: self.model.clone(),
            choices,
            usage,
        };

        push_data(out, &to_value(frame));
    }

    fn close(&mut self, out: &mut String) {
        if self.closed {
            return;
        }
        self.closed = true;

        match self.to {
            Dialect::Anthropic => {
                if self.started {
                    emit_tagged(out, &AnthropicStreamOut::ContentBlockStop { index: 0 });
                    emit_tagged(out, &AnthropicStreamOut::MessageDelta {
                        delta: StopDeltaOut {
                            stop_reason: self.stop_reason.clone().unwrap_or_else(|| "end_turn".into()),
                            stop_sequence: None,
                        },
                        usage: self.usage,
                    });
                }
                emit_tagged(out, &AnthropicStreamOut::MessageStop);
            }
            Dialect::OpenAiCompatible => {
                let finish = self.stop_reason.clone().unwrap_or_else(|| "stop".into());
                self.emit_openai(out, OpenAiDeltaOut::default(), Some(finish), None);

                let usage = OpenAiUsage {
                    prompt_tokens: self.usage.input_tokens,
                    completion_tokens: self.usage.output_tokens,
                    total_tokens: self.usage.input_tokens + self.usage.output_tokens,
                };
                self.emit_openai(out, OpenAiDeltaOut::default(), None, Some(usage));

                out.push_str("data: [DONE]\n\n");
            }
        }
    }

    fn emit_error(&mut self, error: ErrorBody, out: &mut String) {
        match self.to {
            Dialect::Anthropic => {
                out.push_str("event: error\n");
                push_data(out, &to_value(AnthropicErrorEvent { kind: "error", error }));
            }
            Dialect::OpenAiCompatible => push_data(out, &to_value(OpenAiErrorChunk { error })),
        }

        self.closed = true;
    }
}

fn emit_tagged(out: &mut String, event: &AnthropicStreamOut) {
    let value = to_value(event);
    let name = value.get("type").and_then(Value::as_str).unwrap_or("").to_owned();

    out.push_str("event: ");
    out.push_str(&name);
    out.push('\n');

    push_data(out, &value);
}

fn push_data(out: &mut String, data: &Value) {
    out.push_str("data: ");
    out.push_str(&data.to_string());
    out.push_str("\n\n");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect(t: &mut SseTranslator, frames: &[&str]) -> String {
        let mut s = String::new();
        for f in frames {
            s.push_str(&String::from_utf8(t.push(f.as_bytes())).unwrap());
        }
        s.push_str(&String::from_utf8(t.finish()).unwrap());
        s
    }

    #[test]
    fn streams_openai_chunks_into_anthropic_events() {
        let mut t = SseTranslator::new(Dialect::OpenAiCompatible, Dialect::Anthropic, "gpt-4o");
        let out = collect(
            &mut t,
            &[
                "data: {\"id\":\"c1\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\"},\"finish_reason\":null}]}\n\n",
                "data: {\"id\":\"c1\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hi\"},\"finish_reason\":null}]}\n\n",
                "data: {\"id\":\"c1\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
                "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":1}}\n\n",
                "data: [DONE]\n\n",
            ],
        );
        assert!(out.contains("event: message_start"));
        assert!(out.contains("\"text\":\"Hi\""));
        assert!(out.contains("event: message_delta"));
        assert!(out.contains("\"stop_reason\":\"end_turn\""));
        assert!(out.contains("\"output_tokens\":1"));
        assert!(out.contains("event: message_stop"));
    }

    #[test]
    fn streams_anthropic_events_into_openai_chunks() {
        let mut t = SseTranslator::new(Dialect::Anthropic, Dialect::OpenAiCompatible, "claude-opus-4-8");
        let out = collect(
            &mut t,
            &[
                "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"m1\",\"usage\":{\"input_tokens\":5,\"output_tokens\":0}}}\n\n",
                "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hi\"}}\n\n",
                "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":2}}\n\n",
                "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
            ],
        );
        assert!(out.contains("chat.completion.chunk"));
        assert!(out.contains("\"content\":\"Hi\""));
        assert!(out.contains("\"finish_reason\":\"stop\""));
        assert!(out.contains("\"completion_tokens\":2"));
        assert!(out.trim_end().ends_with("data: [DONE]"));
    }

    #[test]
    fn translates_openai_error_chunk_to_anthropic_event() {
        let mut t = SseTranslator::new(Dialect::OpenAiCompatible, Dialect::Anthropic, "gpt-4o");
        let out = collect(
            &mut t,
            &["data: {\"error\":{\"message\":\"overloaded\",\"type\":\"server_error\"}}\n\n"],
        );
        assert!(out.contains("event: error"));
        assert!(out.contains("\"message\":\"overloaded\""));
        assert!(!out.contains("[DONE]"));
    }

    #[test]
    fn translates_anthropic_error_event_to_openai_chunk() {
        let mut t = SseTranslator::new(Dialect::Anthropic, Dialect::OpenAiCompatible, "claude-opus-4-8");
        let out = collect(
            &mut t,
            &["event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"overloaded\"}}\n\n"],
        );
        assert!(out.contains("\"error\""));
        assert!(out.contains("\"message\":\"overloaded\""));
        assert!(!out.contains("[DONE]"));
    }

    #[test]
    fn passthrough_when_dialects_match() {
        let mut t = SseTranslator::new(Dialect::Anthropic, Dialect::Anthropic, "claude-opus-4-8");
        assert!(t.is_passthrough());
        assert_eq!(t.push(b"data: x\n\n"), b"data: x\n\n");
        assert!(t.finish().is_empty());
    }
}
