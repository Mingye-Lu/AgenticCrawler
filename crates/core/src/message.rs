#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageRole {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContentBlock {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: String,
    },
    ToolResult {
        tool_use_id: String,
        tool_name: String,
        output: String,
        is_error: bool,
    },
    Reasoning {
        data: String,
    },
    ToolResultImage {
        tool_use_id: String,
        tool_name: String,
        media_type: String,
        base64_data: String,
        caption: String,
        is_error: bool,
    },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TokenUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cache_creation_input_tokens: u32,
    pub cache_read_input_tokens: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationMessage {
    pub role: MessageRole,
    pub blocks: Vec<ContentBlock>,
    pub usage: Option<TokenUsage>,
}

impl TokenUsage {
    #[must_use]
    pub fn total_tokens(self) -> u32 {
        self.input_tokens
            + self.output_tokens
            + self.cache_creation_input_tokens
            + self.cache_read_input_tokens
    }
}

impl ConversationMessage {
    #[must_use]
    pub fn user_text(text: impl Into<String>) -> Self {
        Self {
            role: MessageRole::User,
            blocks: vec![ContentBlock::Text { text: text.into() }],
            usage: None,
        }
    }

    #[must_use]
    pub fn assistant(blocks: Vec<ContentBlock>) -> Self {
        Self {
            role: MessageRole::Assistant,
            blocks,
            usage: None,
        }
    }

    #[must_use]
    pub fn assistant_with_usage(blocks: Vec<ContentBlock>, usage: Option<TokenUsage>) -> Self {
        Self {
            role: MessageRole::Assistant,
            blocks,
            usage,
        }
    }

    #[must_use]
    pub fn tool_result(
        tool_use_id: impl Into<String>,
        tool_name: impl Into<String>,
        output: impl Into<String>,
        is_error: bool,
    ) -> Self {
        Self {
            role: MessageRole::Tool,
            blocks: vec![ContentBlock::ToolResult {
                tool_use_id: tool_use_id.into(),
                tool_name: tool_name.into(),
                output: output.into(),
                is_error,
            }],
            usage: None,
        }
    }

    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn tool_result_image(
        tool_use_id: impl Into<String>,
        tool_name: impl Into<String>,
        media_type: impl Into<String>,
        base64_data: impl Into<String>,
        caption: impl Into<String>,
        is_error: bool,
    ) -> Self {
        Self {
            role: MessageRole::Tool,
            blocks: vec![ContentBlock::ToolResultImage {
                tool_use_id: tool_use_id.into(),
                tool_name: tool_name.into(),
                media_type: media_type.into(),
                base64_data: base64_data.into(),
                caption: caption.into(),
                is_error,
            }],
            usage: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_result_image_constructor_stores_all_fields() {
        let msg = ConversationMessage::tool_result_image("id1", "screenshot", "image/png", "abc123", "a screenshot", false);
        match &msg.blocks[0] {
            ContentBlock::ToolResultImage { tool_use_id, tool_name, media_type, base64_data, caption, is_error } => {
                assert_eq!(tool_use_id, "id1");
                assert_eq!(tool_name, "screenshot");
                assert_eq!(media_type, "image/png");
                assert_eq!(base64_data, "abc123");
                assert_eq!(caption, "a screenshot");
                assert!(!is_error);
            }
            _ => panic!("expected ToolResultImage"),
        }
    }

    #[test]
    fn vision_payload_debug() {
        let p = crate::effect::VisionPayload {
            base64_data: "abc".to_string(),
            media_type: "image/png".to_string(),
            caption: "test".to_string(),
        };
        assert!(format!("{p:?}").contains("abc"));
    }
}
