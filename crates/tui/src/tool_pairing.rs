//! Tool result pairing utility — maps `tool_use_id` to `ToolResultInfo`.

use std::collections::HashMap;

use acrawl_core::message::{ContentBlock, ConversationMessage, MessageRole};

/// Information about a tool result.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub struct ToolResultInfo {
    /// The output from the tool execution.
    pub output: String,
    /// Whether the tool execution resulted in an error.
    pub is_error: bool,
}

/// Build an index mapping `tool_use_id` to `ToolResultInfo` from a list of messages.
///
/// Scans all messages looking for `role == MessageRole::Tool` with `ContentBlock::ToolResult` blocks.
/// Maps `tool_use_id` → `ToolResultInfo { output, is_error }`.
///
/// # Arguments
///
/// * `messages` - A slice of conversation messages to scan.
///
/// # Returns
///
/// A `HashMap` where keys are `tool_use_id` strings and values are `ToolResultInfo` structs.
#[must_use]
pub fn build_tool_result_index(
    messages: &[ConversationMessage],
) -> HashMap<String, ToolResultInfo> {
    let mut index = HashMap::new();

    for message in messages {
        if message.role == MessageRole::Tool {
            for block in &message.blocks {
                if let ContentBlock::ToolResult {
                    tool_use_id,
                    output,
                    is_error,
                    ..
                } = block
                {
                    index.insert(
                        tool_use_id.clone(),
                        ToolResultInfo {
                            output: output.clone(),
                            is_error: *is_error,
                        },
                    );
                }
            }
        }
    }

    index
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_paired_tool_found_in_index() {
        // ToolUse + ToolResult → index has entry
        let messages = vec![ConversationMessage {
            role: MessageRole::Tool,
            blocks: vec![ContentBlock::ToolResult {
                tool_use_id: "tool_123".to_string(),
                tool_name: "navigate".to_string(),
                output: "Page loaded successfully".to_string(),
                is_error: false,
            }],
            usage: None,
        }];

        let index = build_tool_result_index(&messages);

        assert_eq!(index.len(), 1);
        assert!(index.contains_key("tool_123"));
        let info = &index["tool_123"];
        assert_eq!(info.output, "Page loaded successfully");
        assert!(!info.is_error);
    }

    #[test]
    fn test_unmatched_tool_not_in_index() {
        // ToolUse only (no ToolResult) → NOT in index
        let messages = vec![ConversationMessage {
            role: MessageRole::Assistant,
            blocks: vec![ContentBlock::ToolUse {
                id: "tool_456".to_string(),
                name: "click".to_string(),
                input: r#"{"selector": ".button"}"#.to_string(),
            }],
            usage: None,
        }];

        let index = build_tool_result_index(&messages);

        assert_eq!(index.len(), 0);
        assert!(!index.contains_key("tool_456"));
    }

    #[test]
    fn test_error_tool_flagged() {
        // ToolResult with `is_error: true` → `info.is_error == true`
        let messages = vec![ConversationMessage {
            role: MessageRole::Tool,
            blocks: vec![ContentBlock::ToolResult {
                tool_use_id: "tool_789".to_string(),
                tool_name: "navigate".to_string(),
                output: "Connection timeout".to_string(),
                is_error: true,
            }],
            usage: None,
        }];

        let index = build_tool_result_index(&messages);

        assert_eq!(index.len(), 1);
        let info = &index["tool_789"];
        assert_eq!(info.output, "Connection timeout");
        assert!(info.is_error);
    }

    #[test]
    fn test_empty_messages() {
        // empty vec → empty index
        let messages: Vec<ConversationMessage> = vec![];

        let index = build_tool_result_index(&messages);

        assert_eq!(index.len(), 0);
    }
}
