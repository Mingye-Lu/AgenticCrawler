use serde_json::Value;

/// Control-flow instruction returned by a tool handler.
#[derive(Debug, Clone)]
pub enum ToolEffect {
    /// Tool produced a plain string reply (the most common case).
    Reply(String),
    /// Tool requests spawning a sub-agent with the given spec.
    Spawn(ForkSpec),
    /// Tool requests waiting for sub-agents to finish.
    Wait(WaitSpec),
    /// Tool requests pausing execution for human intervention.
    Pause { reason: String },
}

impl ToolEffect {
    #[must_use]
    pub fn reply_json(value: &Value) -> Self {
        Self::Reply(value.to_string())
    }
}

/// Parameters for spawning a sub-agent.
#[derive(Debug, Clone)]
pub struct ForkSpec {
    pub goal: String,
    pub page_index: Option<usize>,
}

/// Parameters for waiting on sub-agents.
#[derive(Debug, Clone)]
pub struct WaitSpec {
    pub child_ids: Option<Vec<String>>,
}

/// Error type for tool execution failures.
///
/// Most failures are `Message(_)`. The `RequiresAsync` variant is a sentinel
/// emitted by the synchronous handler stub for any tool that actually needs
/// the async path; the agent loop matches on it via [`ToolError::is_requires_async`]
/// instead of substring-matching the error text — which used to misclassify
/// any unrelated error whose message happened to contain the same phrase.
#[derive(Debug)]
pub enum ToolError {
    /// A plain, user-visible error message.
    Message(String),
    /// The named tool is registered as async-only and must go through
    /// `execute_async`. Carries the tool name purely for diagnostics.
    RequiresAsync { tool_name: String },
}

impl ToolError {
    /// Construct a plain message error.
    pub fn new<S: Into<String>>(message: S) -> Self {
        Self::Message(message.into())
    }

    /// Construct the sentinel emitted by the registry stub for async-only tools.
    pub fn requires_async<S: Into<String>>(tool_name: S) -> Self {
        Self::RequiresAsync {
            tool_name: tool_name.into(),
        }
    }

    /// True iff this error is the `RequiresAsync` sentinel.
    #[must_use]
    pub const fn is_requires_async(&self) -> bool {
        matches!(self, Self::RequiresAsync { .. })
    }
}

impl std::fmt::Display for ToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Message(s) => write!(f, "{s}"),
            Self::RequiresAsync { tool_name } => write!(
                f,
                "tool `{tool_name}` requires async execution via execute_async"
            ),
        }
    }
}

impl std::error::Error for ToolError {}

impl From<crate::CrawlError> for ToolError {
    fn from(value: crate::CrawlError) -> Self {
        Self::new(value.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reply_json_serializes_complex_value() {
        let value = serde_json::json!({"items": [1, 2, 3], "nested": {"key": "val"}});
        let effect = ToolEffect::reply_json(&value);
        match effect {
            ToolEffect::Reply(s) => {
                let parsed: serde_json::Value =
                    serde_json::from_str(&s).expect("should be valid JSON");
                assert_eq!(parsed["items"][0], 1);
                assert_eq!(parsed["nested"]["key"], "val");
            }
            _ => panic!("expected Reply variant"),
        }
    }

    #[test]
    fn reply_json_roundtrips_null() {
        let effect = ToolEffect::reply_json(&serde_json::json!(null));
        match effect {
            ToolEffect::Reply(s) => assert_eq!(s, "null"),
            _ => panic!("expected Reply variant"),
        }
    }

    #[test]
    fn tool_error_display_message() {
        let err = ToolError::new("something went wrong");
        assert_eq!(err.to_string(), "something went wrong");
        assert!(!err.is_requires_async());
    }

    #[test]
    fn tool_error_from_crawl_error_conversion() {
        let crawl_err = crate::CrawlError::new("crawl failure");
        let tool_err: ToolError = crawl_err.into();
        assert_eq!(tool_err.to_string(), "crawl failure");
        assert!(!tool_err.is_requires_async());
    }

    #[test]
    fn tool_error_requires_async_variant_is_recognised_by_predicate() {
        let err = ToolError::requires_async("navigate");
        assert!(err.is_requires_async());
        // Display still includes the canonical phrasing so log readers stay
        // oriented even though callers must not substring-match.
        assert!(err
            .to_string()
            .contains("requires async execution via execute_async"));
    }

    #[test]
    fn tool_error_message_containing_marker_phrase_is_not_misclassified() {
        // Pre-refactor, `is_requires_async_error` was a substring-match on
        // `error.to_string()`, which meant any error message that happened
        // to mention the canonical phrase would be misclassified. With a
        // dedicated variant, the predicate is identity-based, not text-based.
        let err = ToolError::new(
            "upstream said: tool `foo` requires async execution via execute_async (but it's a Message)",
        );
        assert!(
            !err.is_requires_async(),
            "Message variant must not be reported as RequiresAsync regardless of its text"
        );
    }

    #[test]
    fn pause_variant_has_reason() {
        let effect = ToolEffect::Pause {
            reason: "test reason".to_string(),
        };
        match effect {
            ToolEffect::Pause { reason } => assert_eq!(reason, "test reason"),
            _ => panic!("expected Pause variant"),
        }
    }
}
