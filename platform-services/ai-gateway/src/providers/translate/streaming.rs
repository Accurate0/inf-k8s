use llm_bridge_core::model::{ApiFormat, StreamState};
use llm_bridge_core::transform::{transform_stream, transform_stream_to_openai};

use super::Dialect;

/// Incrementally reshapes an upstream SSE byte stream from one dialect into the other,
/// delegating the protocol conversion to `llm-bridge-core`. [`push`](Self::push) returns
/// translated SSE bytes to forward as complete frames arrive; [`finish`](Self::finish)
/// flushes any trailing frame. A matching-dialect translator passes bytes through
/// unchanged.
pub struct SseTranslator {
    source: ApiFormat,
    target: Dialect,
    state: StreamState,
    pending: Vec<u8>,
    passthrough: bool,
}

impl SseTranslator {
    pub fn new(from: Dialect, to: Dialect, _model: &str) -> Self {
        Self {
            source: from.api_format(),
            target: to,
            state: StreamState::default(),
            pending: Vec::new(),
            passthrough: from == to,
        }
    }

    pub fn is_passthrough(&self) -> bool {
        self.passthrough
    }

    pub fn push(&mut self, chunk: &[u8]) -> Vec<u8> {
        if self.passthrough {
            return chunk.to_vec();
        }

        self.pending.extend_from_slice(chunk);
        match take_complete_frames(&mut self.pending) {
            Some(frames) => self.transform(&frames),
            None => Vec::new(),
        }
    }

    pub fn finish(&mut self) -> Vec<u8> {
        if self.passthrough || self.pending.is_empty() {
            return Vec::new();
        }
        let rest = std::mem::take(&mut self.pending);
        self.transform(&rest)
    }

    fn transform(&mut self, frames: &[u8]) -> Vec<u8> {
        let result = match self.target {
            Dialect::Anthropic => transform_stream(frames, self.source, &mut self.state),
            Dialect::OpenAiCompatible => {
                transform_stream_to_openai(frames, self.source, &mut self.state)
            }
        };
        result.unwrap_or_default()
    }
}

/// Drains every byte up to and including the last complete SSE frame boundary (`\n\n`),
/// leaving any partial trailing frame buffered. Returns `None` when no frame is complete.
fn take_complete_frames(pending: &mut Vec<u8>) -> Option<Vec<u8>> {
    let boundary = pending
        .windows(2)
        .enumerate()
        .rfind(|(_, w)| *w == b"\n\n")
        .map(|(i, _)| i + 2)?;
    Some(pending.drain(..boundary).collect())
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
                "data: {\"id\":\"c1\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":1}}\n\n",
                "data: [DONE]\n\n",
            ],
        );
        assert!(out.contains("message_start"));
        assert!(out.contains("Hi"));
        assert!(out.contains("message_stop"));
    }

    #[test]
    fn streams_anthropic_events_into_openai_chunks() {
        let mut t = SseTranslator::new(
            Dialect::Anthropic,
            Dialect::OpenAiCompatible,
            "claude-opus-4-8",
        );
        let out = collect(
            &mut t,
            &[
                "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"m1\",\"model\":\"claude-opus-4-8\",\"role\":\"assistant\",\"content\":[],\"usage\":{\"input_tokens\":5,\"output_tokens\":0}}}\n\n",
                "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
                "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hi\"}}\n\n",
                "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
                "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":2}}\n\n",
                "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
            ],
        );
        assert!(out.contains("chat.completion.chunk"));
        assert!(out.contains("Hi"));
        assert!(out.contains("[DONE]"));
    }

    #[test]
    fn passthrough_when_dialects_match() {
        let mut t = SseTranslator::new(Dialect::Anthropic, Dialect::Anthropic, "claude-opus-4-8");
        assert!(t.is_passthrough());
        assert_eq!(t.push(b"data: x\n\n"), b"data: x\n\n");
        assert!(t.finish().is_empty());
    }
}
