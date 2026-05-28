use crate::message::TokenUsage;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssistantEvent {
    TextDelta(String),
    ToolUse {
        id: String,
        name: String,
        input: String,
    },
    Reasoning {
        data: String,
    },
    Usage(TokenUsage),
    MessageStop,
}
