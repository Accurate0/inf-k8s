use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub(super) const DEFAULT_MAX_TOKENS: i64 = 4096;

#[derive(Deserialize)]
#[serde(untagged)]
pub(super) enum Content {
    Text(String),
    Parts(Vec<Part>),
    Other(serde::de::IgnoredAny),
}

impl Default for Content {
    fn default() -> Self {
        Content::Text(String::new())
    }
}

impl Content {
    pub(super) fn to_text(&self) -> String {
        match self {
            Content::Text(s) => s.clone(),
            Content::Parts(parts) => parts_to_text(parts),
            Content::Other(_) => String::new(),
        }
    }
}

#[derive(Deserialize, Serialize)]
pub(super) struct Part {
    #[serde(rename = "type")]
    pub(super) kind: Option<String>,
    #[serde(default)]
    pub(super) text: Option<String>,
}

pub(super) fn parts_to_text(parts: &[Part]) -> String {
    parts
        .iter()
        .filter(|p| p.kind.as_deref() == Some("text"))
        .filter_map(|p| p.text.clone())
        .collect()
}

fn default_role() -> String {
    "user".into()
}

fn opt_text(content: &Option<Content>) -> Option<String> {
    content
        .as_ref()
        .map(Content::to_text)
        .filter(|s| !s.is_empty())
}

fn args_to_input(arguments: &str) -> Value {
    serde_json::from_str(arguments).unwrap_or_else(|_| json!({}))
}

fn value_to_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|b| b.get("text").and_then(Value::as_str).map(str::to_owned))
            .collect(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

#[derive(Deserialize, Serialize, Clone)]
pub(super) struct ToolCall {
    pub(super) id: String,
    #[serde(rename = "type", default = "function_kind")]
    pub(super) kind: String,
    pub(super) function: FunctionCall,
}

fn function_kind() -> String {
    "function".into()
}

#[derive(Deserialize, Serialize, Clone)]
pub(super) struct FunctionCall {
    pub(super) name: String,
    #[serde(default)]
    pub(super) arguments: String,
}

impl ToolCall {
    fn into_tool_use(self) -> OutBlock {
        OutBlock::ToolUse {
            id: self.id,
            name: self.function.name,
            input: args_to_input(&self.function.arguments),
        }
    }
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Block {
    Text {
        #[serde(default)]
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        #[serde(default)]
        input: Value,
    },
    ToolResult {
        tool_use_id: String,
        #[serde(default)]
        content: Value,
    },
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum AnthropicContent {
    Text(String),
    Blocks(Vec<Block>),
    Other(serde::de::IgnoredAny),
}

impl Default for AnthropicContent {
    fn default() -> Self {
        AnthropicContent::Text(String::new())
    }
}

impl AnthropicContent {
    fn into_blocks(self) -> Vec<Block> {
        match self {
            AnthropicContent::Text(s) => vec![Block::Text { text: s }],
            AnthropicContent::Blocks(b) => b,
            AnthropicContent::Other(_) => vec![],
        }
    }
}

#[derive(Deserialize)]
struct AnthropicMessage {
    #[serde(default = "default_role")]
    role: String,
    #[serde(default)]
    content: AnthropicContent,
}

#[derive(Deserialize)]
struct OpenAiMessage {
    #[serde(default = "default_role")]
    role: String,
    #[serde(default)]
    content: Option<Content>,
    #[serde(default)]
    tool_calls: Vec<ToolCall>,
    #[serde(default)]
    tool_call_id: Option<String>,
}

#[derive(Serialize)]
pub(super) struct OpenAiOutMessage {
    pub(super) role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) content: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) tool_calls: Vec<ToolCall>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) tool_call_id: Option<String>,
}

#[derive(Serialize)]
struct AnthropicOutMessage {
    role: String,
    content: Vec<OutBlock>,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum OutBlock {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
    },
}

#[derive(Deserialize)]
struct AnthropicTool {
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    input_schema: Value,
}

#[derive(Deserialize)]
struct OpenAiTool {
    #[serde(default)]
    function: OpenAiToolFunction,
}

#[derive(Deserialize, Default)]
struct OpenAiToolFunction {
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    parameters: Value,
}

#[derive(Serialize)]
struct OpenAiToolOut {
    #[serde(rename = "type")]
    kind: &'static str,
    function: OpenAiToolFunctionOut,
}

#[derive(Serialize)]
struct OpenAiToolFunctionOut {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    parameters: Value,
}

impl From<AnthropicTool> for OpenAiToolOut {
    fn from(t: AnthropicTool) -> Self {
        OpenAiToolOut {
            kind: "function",
            function: OpenAiToolFunctionOut {
                name: t.name,
                description: t.description,
                parameters: t.input_schema,
            },
        }
    }
}

#[derive(Serialize)]
struct AnthropicToolOut {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    input_schema: Value,
}

impl From<OpenAiTool> for AnthropicToolOut {
    fn from(t: OpenAiTool) -> Self {
        AnthropicToolOut {
            name: t.function.name,
            description: t.function.description,
            input_schema: t.function.parameters,
        }
    }
}

fn tool_choice_a2o(v: Value) -> Value {
    match v.get("type").and_then(Value::as_str) {
        Some("any") => json!("required"),
        Some("none") => json!("none"),
        Some("tool") => json!({ "type": "function", "function": { "name": v.get("name") } }),
        _ => json!("auto"),
    }
}

fn tool_choice_o2a(v: Value) -> Value {
    match &v {
        Value::String(s) => match s.as_str() {
            "required" => json!({ "type": "any" }),
            "none" => json!({ "type": "none" }),
            _ => json!({ "type": "auto" }),
        },
        Value::Object(_) => {
            json!({ "type": "tool", "name": v.get("function").and_then(|f| f.get("name")) })
        }
        _ => json!({ "type": "auto" }),
    }
}

#[derive(Deserialize, Serialize, Default, Clone, Copy)]
pub(super) struct AnthropicUsage {
    #[serde(default)]
    pub(super) input_tokens: i64,
    #[serde(default)]
    pub(super) output_tokens: i64,
}

#[derive(Deserialize, Serialize, Default, Clone, Copy)]
pub(super) struct OpenAiUsage {
    #[serde(default)]
    pub(super) prompt_tokens: i64,
    #[serde(default)]
    pub(super) completion_tokens: i64,
    #[serde(default)]
    pub(super) total_tokens: i64,
}

#[derive(Deserialize, Default)]
pub(super) struct AnthropicRequest {
    model: Option<String>,
    system: Option<Content>,
    #[serde(default)]
    messages: Vec<AnthropicMessage>,
    max_tokens: Option<i64>,
    stop_sequences: Option<Vec<String>>,
    temperature: Option<f64>,
    top_p: Option<f64>,
    stream: Option<bool>,
    tools: Option<Vec<AnthropicTool>>,
    tool_choice: Option<Value>,
}

#[derive(Deserialize, Default)]
pub(super) struct OpenAiRequest {
    model: Option<String>,
    #[serde(default)]
    messages: Vec<OpenAiMessage>,
    max_tokens: Option<i64>,
    max_completion_tokens: Option<i64>,
    stop: Option<StopField>,
    temperature: Option<f64>,
    top_p: Option<f64>,
    stream: Option<bool>,
    tools: Option<Vec<OpenAiTool>>,
    tool_choice: Option<Value>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum StopField {
    One(String),
    Many(Vec<String>),
}

#[derive(Serialize)]
struct StreamOptions {
    include_usage: bool,
}

#[derive(Serialize)]
pub(super) struct OpenAiChatRequest {
    model: Option<String>,
    messages: Vec<OpenAiOutMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stop: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream_options: Option<StreamOptions>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<OpenAiToolOut>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<Value>,
}

impl From<AnthropicRequest> for OpenAiChatRequest {
    fn from(a: AnthropicRequest) -> Self {
        let mut messages = Vec::new();

        if let Some(system) = a
            .system
            .as_ref()
            .map(Content::to_text)
            .filter(|s| !s.is_empty())
        {
            messages.push(OpenAiOutMessage {
                role: "system".into(),
                content: Some(system),
                tool_calls: vec![],
                tool_call_id: None,
            });
        }

        for m in a.messages {
            let mut text = String::new();
            let mut tool_calls = Vec::new();
            let mut tool_results = Vec::new();

            for block in m.content.into_blocks() {
                match block {
                    Block::Text { text: t } => text.push_str(&t),
                    Block::ToolUse { id, name, input } => tool_calls.push(ToolCall {
                        id,
                        kind: "function".into(),
                        function: FunctionCall {
                            name,
                            arguments: input.to_string(),
                        },
                    }),
                    Block::ToolResult {
                        tool_use_id,
                        content,
                    } => tool_results.push((tool_use_id, value_to_text(&content))),
                    Block::Other => {}
                }
            }

            for (tool_call_id, content) in tool_results {
                messages.push(OpenAiOutMessage {
                    role: "tool".into(),
                    content: Some(content),
                    tool_calls: vec![],
                    tool_call_id: Some(tool_call_id),
                });
            }

            if !text.is_empty() || !tool_calls.is_empty() {
                messages.push(OpenAiOutMessage {
                    role: m.role,
                    content: (!text.is_empty()).then_some(text),
                    tool_calls,
                    tool_call_id: None,
                });
            }
        }

        let stream_options = a.stream.unwrap_or(false).then_some(StreamOptions {
            include_usage: true,
        });

        OpenAiChatRequest {
            model: a.model,
            messages,
            max_tokens: a.max_tokens,
            stop: a.stop_sequences,
            temperature: a.temperature,
            top_p: a.top_p,
            stream: a.stream,
            stream_options,
            tools: a
                .tools
                .map(|ts| ts.into_iter().map(OpenAiToolOut::from).collect()),
            tool_choice: a.tool_choice.map(tool_choice_a2o),
        }
    }
}

#[derive(Serialize)]
pub(super) struct AnthropicMessagesRequest {
    model: Option<String>,
    messages: Vec<AnthropicOutMessage>,
    max_tokens: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stop_sequences: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<AnthropicToolOut>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<Value>,
}

impl From<OpenAiRequest> for AnthropicMessagesRequest {
    fn from(o: OpenAiRequest) -> Self {
        let mut system = String::new();
        let mut chat = Vec::new();

        for m in o.messages {
            if m.role == "system" {
                if !system.is_empty() {
                    system.push_str("\n\n");
                }
                system.push_str(&m.content.as_ref().map(Content::to_text).unwrap_or_default());
            } else {
                chat.push(m);
            }
        }

        let mut messages = Vec::new();
        let mut iter = chat.into_iter().peekable();

        while let Some(m) = iter.next() {
            match m.role.as_str() {
                "tool" => {
                    let mut blocks = vec![tool_result_block(m)];
                    while iter.peek().is_some_and(|n| n.role == "tool") {
                        blocks.push(tool_result_block(iter.next().unwrap()));
                    }
                    messages.push(AnthropicOutMessage {
                        role: "user".into(),
                        content: blocks,
                    });
                }
                "assistant" => {
                    let mut blocks = Vec::new();
                    if let Some(text) = opt_text(&m.content) {
                        blocks.push(OutBlock::Text { text });
                    }
                    for tc in m.tool_calls {
                        blocks.push(tc.into_tool_use());
                    }
                    messages.push(AnthropicOutMessage {
                        role: "assistant".into(),
                        content: blocks,
                    });
                }
                _ => messages.push(AnthropicOutMessage {
                    role: m.role,
                    content: vec![OutBlock::Text {
                        text: m.content.as_ref().map(Content::to_text).unwrap_or_default(),
                    }],
                }),
            }
        }

        let stop_sequences = o.stop.map(|s| match s {
            StopField::One(x) => vec![x],
            StopField::Many(v) => v,
        });

        AnthropicMessagesRequest {
            model: o.model,
            messages,
            max_tokens: o
                .max_tokens
                .or(o.max_completion_tokens)
                .unwrap_or(DEFAULT_MAX_TOKENS),
            system: (!system.is_empty()).then_some(system),
            stop_sequences,
            temperature: o.temperature,
            top_p: o.top_p,
            stream: o.stream,
            tools: o
                .tools
                .map(|ts| ts.into_iter().map(AnthropicToolOut::from).collect()),
            tool_choice: o.tool_choice.map(tool_choice_o2a),
        }
    }
}

fn tool_result_block(m: OpenAiMessage) -> OutBlock {
    OutBlock::ToolResult {
        tool_use_id: m.tool_call_id.unwrap_or_default(),
        content: m.content.as_ref().map(Content::to_text).unwrap_or_default(),
    }
}

#[derive(Deserialize, Default)]
pub(super) struct OpenAiResponse {
    id: Option<String>,
    model: Option<String>,
    #[serde(default)]
    choices: Vec<OpenAiChoice>,
    usage: Option<OpenAiUsage>,
}

#[derive(Deserialize, Default)]
struct OpenAiChoice {
    #[serde(default)]
    message: RespMessage,
    finish_reason: Option<String>,
}

#[derive(Deserialize, Default)]
struct RespMessage {
    #[serde(default)]
    content: Option<Content>,
    #[serde(default)]
    tool_calls: Vec<ToolCall>,
}

#[derive(Deserialize, Default)]
pub(super) struct AnthropicResponse {
    id: Option<String>,
    model: Option<String>,
    #[serde(default)]
    content: Vec<Block>,
    stop_reason: Option<String>,
    usage: Option<AnthropicUsage>,
}

#[derive(Serialize)]
pub(super) struct AnthropicMessageResponse {
    id: Option<String>,
    #[serde(rename = "type")]
    kind: &'static str,
    role: &'static str,
    model: Option<String>,
    content: Vec<OutBlock>,
    stop_reason: &'static str,
    stop_sequence: Option<String>,
    usage: AnthropicUsage,
}

impl From<OpenAiResponse> for AnthropicMessageResponse {
    fn from(o: OpenAiResponse) -> Self {
        let choice = o.choices.into_iter().next().unwrap_or_default();
        let usage = o.usage.unwrap_or_default();

        let mut content = Vec::new();
        if let Some(text) = opt_text(&choice.message.content) {
            content.push(OutBlock::Text { text });
        }
        for tc in choice.message.tool_calls {
            content.push(tc.into_tool_use());
        }
        if content.is_empty() {
            content.push(OutBlock::Text {
                text: String::new(),
            });
        }

        AnthropicMessageResponse {
            id: o.id,
            kind: "message",
            role: "assistant",
            model: o.model,
            content,
            stop_reason: openai_finish_to_anthropic(choice.finish_reason.as_deref()),
            stop_sequence: None,
            usage: AnthropicUsage {
                input_tokens: usage.prompt_tokens,
                output_tokens: usage.completion_tokens,
            },
        }
    }
}

#[derive(Serialize)]
pub(super) struct OpenAiChatResponse {
    id: Option<String>,
    object: &'static str,
    created: i64,
    model: Option<String>,
    choices: Vec<OutChoice>,
    usage: OpenAiUsage,
}

#[derive(Serialize)]
struct OutChoice {
    index: u32,
    message: OpenAiOutMessage,
    finish_reason: &'static str,
}

impl From<AnthropicResponse> for OpenAiChatResponse {
    fn from(a: AnthropicResponse) -> Self {
        let usage = a.usage.unwrap_or_default();

        let mut text = String::new();
        let mut tool_calls = Vec::new();
        for block in a.content {
            match block {
                Block::Text { text: t } => text.push_str(&t),
                Block::ToolUse { id, name, input } => tool_calls.push(ToolCall {
                    id,
                    kind: "function".into(),
                    function: FunctionCall {
                        name,
                        arguments: input.to_string(),
                    },
                }),
                _ => {}
            }
        }

        let message = OpenAiOutMessage {
            role: "assistant".into(),
            content: (!text.is_empty()).then_some(text),
            tool_calls,
            tool_call_id: None,
        };

        OpenAiChatResponse {
            id: a.id,
            object: "chat.completion",
            created: chrono::Utc::now().timestamp(),
            model: a.model,
            choices: vec![OutChoice {
                index: 0,
                message,
                finish_reason: anthropic_stop_to_openai(a.stop_reason.as_deref()),
            }],
            usage: OpenAiUsage {
                prompt_tokens: usage.input_tokens,
                completion_tokens: usage.output_tokens,
                total_tokens: usage.input_tokens + usage.output_tokens,
            },
        }
    }
}

pub(super) fn openai_finish_to_anthropic(finish: Option<&str>) -> &'static str {
    match finish {
        Some("length") => "max_tokens",
        Some("tool_calls") | Some("function_call") => "tool_use",
        _ => "end_turn",
    }
}

pub(super) fn anthropic_stop_to_openai(stop: Option<&str>) -> &'static str {
    match stop {
        Some("max_tokens") => "length",
        Some("tool_use") => "tool_calls",
        _ => "stop",
    }
}

#[derive(Deserialize)]
pub(super) struct OpenAiStreamChunk {
    pub(super) id: Option<String>,
    #[serde(default)]
    pub(super) choices: Vec<OpenAiStreamChoice>,
    pub(super) usage: Option<OpenAiUsage>,
}

#[derive(Deserialize, Default)]
pub(super) struct OpenAiStreamChoice {
    #[serde(default)]
    pub(super) delta: OpenAiDelta,
    pub(super) finish_reason: Option<String>,
}

#[derive(Deserialize, Default)]
pub(super) struct OpenAiDelta {
    pub(super) content: Option<String>,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum AnthropicStreamEvent {
    MessageStart {
        message: StartMessage,
    },
    ContentBlockDelta {
        delta: TextDelta,
    },
    MessageDelta {
        delta: StopDelta,
        usage: Option<AnthropicUsage>,
    },
    MessageStop,
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
pub(super) struct StartMessage {
    pub(super) id: Option<String>,
    pub(super) usage: Option<AnthropicUsage>,
}

#[derive(Deserialize)]
pub(super) struct TextDelta {
    pub(super) text: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct StopDelta {
    pub(super) stop_reason: Option<String>,
}

#[derive(Serialize)]
pub(super) struct OpenAiStreamOut {
    pub(super) id: String,
    pub(super) object: &'static str,
    pub(super) model: String,
    pub(super) choices: Vec<OpenAiStreamOutChoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) usage: Option<OpenAiUsage>,
}

#[derive(Serialize)]
pub(super) struct OpenAiStreamOutChoice {
    pub(super) index: u32,
    pub(super) delta: OpenAiDeltaOut,
    pub(super) finish_reason: Option<String>,
}

#[derive(Serialize, Default)]
pub(super) struct OpenAiDeltaOut {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) content: Option<String>,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum AnthropicStreamOut {
    MessageStart {
        message: OutStartMessage,
    },
    ContentBlockStart {
        index: u32,
        content_block: OutBlock,
    },
    ContentBlockDelta {
        index: u32,
        delta: TextDeltaOut,
    },
    ContentBlockStop {
        index: u32,
    },
    MessageDelta {
        delta: StopDeltaOut,
        usage: AnthropicUsage,
    },
    MessageStop,
}

#[derive(Serialize)]
pub(super) struct OutStartMessage {
    pub(super) id: String,
    #[serde(rename = "type")]
    pub(super) kind: &'static str,
    pub(super) role: &'static str,
    pub(super) model: String,
    pub(super) content: Vec<OutBlock>,
    pub(super) stop_reason: Option<String>,
    pub(super) usage: AnthropicUsage,
}

#[derive(Serialize)]
pub(super) struct TextDeltaOut {
    #[serde(rename = "type")]
    pub(super) kind: &'static str,
    pub(super) text: String,
}

#[derive(Serialize)]
pub(super) struct StopDeltaOut {
    pub(super) stop_reason: String,
    pub(super) stop_sequence: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct StreamError {
    pub(super) error: ErrorBody,
}

#[derive(Deserialize, Serialize, Default)]
pub(super) struct ErrorBody {
    #[serde(default)]
    pub(super) message: String,
    #[serde(rename = "type", default)]
    pub(super) kind: String,
}

#[derive(Serialize)]
pub(super) struct AnthropicErrorEvent {
    #[serde(rename = "type")]
    pub(super) kind: &'static str,
    pub(super) error: ErrorBody,
}

#[derive(Serialize)]
pub(super) struct OpenAiErrorChunk {
    pub(super) error: ErrorBody,
}

pub(super) fn to_value<T: Serialize>(v: T) -> Value {
    serde_json::to_value(v).unwrap_or(Value::Null)
}
