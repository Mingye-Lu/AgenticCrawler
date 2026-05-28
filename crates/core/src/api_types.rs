use crate::error::RuntimeError;
use crate::event::AssistantEvent;
use crate::message::ConversationMessage;

/// Request payload for the LLM API.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiRequest {
    pub system_prompt: Vec<String>,
    pub messages: Vec<ConversationMessage>,
}

/// Trait for making LLM API calls.
pub trait ApiClient {
    fn stream(&mut self, request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError>;
}
