pub mod anthropic;
pub mod bedrock;
pub mod catalog;
pub mod custom;
mod factory;
pub mod openai;
pub mod preset;
mod registry;
pub mod transform;

pub use registry::*;

use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::types::{MessageRequest, MessageResponse, ReasoningEffort, StreamEvent};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub display_name: String,
    pub aliases: Vec<String>,
    pub provider_id: String,
    pub max_output_tokens: u32,
    pub context_window: u32,
    pub capabilities: ModelCapabilities,
    pub pricing: Option<ModelPricing>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(clippy::struct_excessive_bools)]
pub struct ModelCapabilities {
    pub reasoning: bool,
    pub tool_use: bool,
    pub vision: bool,
    pub streaming: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reasoning_efforts: Vec<ReasoningEffort>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelPricing {
    pub input_per_mtok: f64,
    pub output_per_mtok: f64,
    pub cache_read_per_mtok: Option<f64>,
    pub cache_write_per_mtok: Option<f64>,
}

pub enum ProviderStream {
    Anthropic(crate::client::MessageStream),
    Bedrock(crate::bedrock::BedrockMessageStream),
    OpenAi(crate::responses::ResponsesMessageStream),
    Custom(crate::openai::OpenAiMessageStream),
    Gemini(crate::gemini::GeminiMessageStream),
}

impl ProviderStream {
    pub async fn next_event(&mut self) -> Result<Option<StreamEvent>, ApiError> {
        match self {
            Self::Anthropic(s) => s.next_event().await,
            Self::Bedrock(s) => s.next_event().await,
            Self::OpenAi(s) => s.next_event().await,
            Self::Custom(s) => s.next_event().await,
            Self::Gemini(s) => s.next_event().await,
        }
    }
}

pub enum ProviderClient {
    Anthropic(crate::AnthropicClient),
    Bedrock(crate::bedrock::BedrockClient),
    OpenAi(crate::OpenAiResponsesClient),
    Custom(crate::ChatCompletionsClient),
    Gemini(crate::gemini::GeminiClient),
}

impl ProviderClient {
    pub async fn stream_message(
        &self,
        request: &MessageRequest,
    ) -> Result<ProviderStream, ApiError> {
        match self {
            Self::Anthropic(c) => c
                .stream_message(request)
                .await
                .map(ProviderStream::Anthropic),
            Self::Bedrock(c) => c.stream_message(request).await.map(ProviderStream::Bedrock),
            Self::OpenAi(c) => c.stream_message(request).await.map(ProviderStream::OpenAi),
            Self::Custom(c) => c.stream_message(request).await.map(ProviderStream::Custom),
            Self::Gemini(c) => c.stream_message(request).await.map(ProviderStream::Gemini),
        }
    }

    pub async fn send_message(
        &self,
        request: &MessageRequest,
    ) -> Result<MessageResponse, ApiError> {
        match self {
            Self::Anthropic(c) => c.send_message(request).await,
            Self::Bedrock(_) => Err(ApiError::Auth(
                "send_message not supported for Bedrock streaming client".into(),
            )),
            Self::Gemini(_) => Err(ApiError::Auth(
                "send_message not supported for Gemini".into(),
            )),
            _ => Err(ApiError::Auth(
                "send_message only supported for Anthropic".into(),
            )),
        }
    }

    #[must_use]
    pub fn is_anthropic(&self) -> bool {
        matches!(self, Self::Anthropic(_) | Self::Bedrock(_))
    }

    #[must_use]
    pub fn supports_send_message(&self) -> bool {
        matches!(self, Self::Anthropic(_))
    }
}
