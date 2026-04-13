use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MessageRequest {
    pub model: String,
    pub max_tokens: u32,
    pub messages: Vec<InputMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolDefinition>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<ToolChoice>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stream: bool,
}

impl MessageRequest {
    #[must_use]
    pub fn with_streaming(mut self) -> Self {
        self.stream = true;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InputMessage {
    pub role: String,
    pub content: Vec<InputContentBlock>,
}

impl InputMessage {
    #[must_use]
    pub fn user_text(text: impl Into<String>) -> Self {
        Self {
            role: "user".to_string(),
            content: vec![InputContentBlock::Text { text: text.into() }],
        }
    }

    #[must_use]
    pub fn user_tool_result(
        tool_use_id: impl Into<String>,
        content: impl Into<String>,
        is_error: bool,
    ) -> Self {
        Self {
            role: "user".to_string(),
            content: vec![InputContentBlock::ToolResult {
                tool_use_id: tool_use_id.into(),
                content: vec![ToolResultContentBlock::Text {
                    text: content.into(),
                }],
                is_error,
            }],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InputContentBlock {
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
        content: Vec<ToolResultContentBlock>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        is_error: bool,
    },
    Reasoning {
        #[serde(flatten)]
        data: Value,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolResultContentBlock {
    Text { text: String },
    Json { value: Value },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub input_schema: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolChoice {
    Auto,
    Any,
    Tool { name: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MessageResponse {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub role: String,
    pub content: Vec<OutputContentBlock>,
    pub model: String,
    #[serde(default)]
    pub stop_reason: Option<String>,
    #[serde(default)]
    pub stop_sequence: Option<String>,
    pub usage: Usage,
    #[serde(default)]
    pub request_id: Option<String>,
}

impl MessageResponse {
    #[must_use]
    pub fn total_tokens(&self) -> u32 {
        self.usage.total_tokens()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OutputContentBlock {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u32,
    #[serde(default)]
    pub cache_creation_input_tokens: u32,
    #[serde(default)]
    pub cache_read_input_tokens: u32,
    pub output_tokens: u32,
}

impl Usage {
    #[must_use]
    pub const fn total_tokens(&self) -> u32 {
        self.input_tokens + self.output_tokens
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MessageStartEvent {
    pub message: MessageResponse,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MessageDeltaEvent {
    pub delta: MessageDelta,
    pub usage: Usage,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageDelta {
    #[serde(default)]
    pub stop_reason: Option<String>,
    #[serde(default)]
    pub stop_sequence: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContentBlockStartEvent {
    pub index: u32,
    pub content_block: OutputContentBlock,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContentBlockDeltaEvent {
    pub index: u32,
    pub delta: ContentBlockDelta,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlockDelta {
    TextDelta { text: String },
    InputJsonDelta { partial_json: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentBlockStopEvent {
    pub index: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageStopEvent {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reasoning_variant_round_trip() {
        let reasoning_data = serde_json::json!({
            "id": "rs_abc123",
            "content": []
        });

        let reasoning_block = InputContentBlock::Reasoning {
            data: reasoning_data.clone(),
        };

        let serialized = serde_json::to_value(&reasoning_block).expect("serialize");

        let deserialized: InputContentBlock =
            serde_json::from_value(serialized.clone()).expect("deserialize");

        assert_eq!(reasoning_block, deserialized);

        assert_eq!(serialized["type"], "reasoning");
        assert_eq!(serialized["id"], "rs_abc123");
    }

    #[test]
    fn reasoning_variant_has_correct_type_field() {
        let reasoning_block = InputContentBlock::Reasoning {
            data: serde_json::json!({
                "id": "rs_xyz",
                "content": []
            }),
        };

        let serialized = serde_json::to_string(&reasoning_block).expect("serialize to string");

        assert!(serialized.contains("\"type\":\"reasoning\""));
    }

    #[test]
    fn existing_text_variant_unchanged() {
        let text_block = InputContentBlock::Text {
            text: "hello world".to_string(),
        };

        let serialized = serde_json::to_value(&text_block).expect("serialize");
        let deserialized: InputContentBlock =
            serde_json::from_value(serialized.clone()).expect("deserialize");

        assert_eq!(text_block, deserialized);
        assert_eq!(serialized["type"], "text");
        assert_eq!(serialized["text"], "hello world");
    }

    #[test]
    fn existing_tool_use_variant_unchanged() {
        let tool_use_block = InputContentBlock::ToolUse {
            id: "call_123".to_string(),
            name: "navigate".to_string(),
            input: serde_json::json!({"url": "https://example.com"}),
        };

        let serialized = serde_json::to_value(&tool_use_block).expect("serialize");
        let deserialized: InputContentBlock =
            serde_json::from_value(serialized.clone()).expect("deserialize");

        assert_eq!(tool_use_block, deserialized);
        assert_eq!(serialized["type"], "tool_use");
        assert_eq!(serialized["id"], "call_123");
        assert_eq!(serialized["name"], "navigate");
    }

    #[test]
    fn reasoning_variant_deserializes_from_json() {
        let json_str = r#"{"type":"reasoning","id":"rs_test","content":[]}"#;
        let deserialized: InputContentBlock =
            serde_json::from_str(json_str).expect("deserialize from json string");

        match deserialized {
            InputContentBlock::Reasoning { data } => {
                assert_eq!(data["id"], "rs_test");
                assert_eq!(data["content"], serde_json::json!([]));
            }
            _ => panic!("Expected Reasoning variant"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamEvent {
    MessageStart(MessageStartEvent),
    MessageDelta(MessageDeltaEvent),
    ContentBlockStart(ContentBlockStartEvent),
    ContentBlockDelta(ContentBlockDeltaEvent),
    ContentBlockStop(ContentBlockStopEvent),
    MessageStop(MessageStopEvent),
}
