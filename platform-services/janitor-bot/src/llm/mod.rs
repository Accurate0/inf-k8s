use anyhow::Context;
use async_openai::Client;
use async_openai::config::OpenAIConfig;
use async_openai::types::chat::{
    ChatCompletionRequestMessage, ChatCompletionResponseMessage, ChatCompletionTools,
    CreateChatCompletionRequestArgs,
};

const DEFAULT_GATEWAY_URL: &str = "http://api.ai-gateway.svc.cluster.local";

/// A thin wrapper around the OpenAI-compatible AI gateway. It holds no
/// domain knowledge — callers supply their own messages, tools, and model.
pub struct LlmClient {
    client: Client<OpenAIConfig>,
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
        let config = OpenAIConfig::new()
            .with_api_base(format!("{}/v1", base_url.trim_end_matches('/')))
            .with_api_key(token);

        Self {
            client: Client::with_config(config),
        }
    }

    /// Sends one chat-completion turn and returns the assistant message.
    pub async fn chat(
        &self,
        model: &str,
        messages: Vec<ChatCompletionRequestMessage>,
        tools: Vec<ChatCompletionTools>,
    ) -> anyhow::Result<ChatCompletionResponseMessage> {
        let request = CreateChatCompletionRequestArgs::default()
            .model(model)
            .temperature(0.0)
            .tools(tools)
            .messages(messages)
            .build()?;

        let resp = self.client.chat().create(request).await?;
        resp.choices
            .into_iter()
            .next()
            .map(|c| c.message)
            .context("LLM returned no choices")
    }
}
