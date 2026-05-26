// Re-export from core for backward compatibility
pub use acrawl_core::error::ToolExecutionError;
pub use acrawl_core::effect::{CancelSpec, CrawlScope, CrawlTask, StatusSpec, ToolEffect, WaitSpec};

impl From<crate::CrawlError> for ToolExecutionError {
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
        let err = ToolExecutionError::new("something went wrong");
        assert_eq!(err.to_string(), "something went wrong");
        assert!(!err.is_requires_async());
    }

    #[test]
    fn tool_error_from_crawl_error_conversion() {
        let crawl_err = crate::CrawlError::new("crawl failure");
        let tool_err: ToolExecutionError = crawl_err.into();
        assert_eq!(tool_err.to_string(), "crawl failure");
        assert!(!tool_err.is_requires_async());
    }

    #[test]
    fn tool_error_requires_async_variant_is_recognised_by_predicate() {
        let err = ToolExecutionError::requires_async("navigate");
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
        let err = ToolExecutionError::new(
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
