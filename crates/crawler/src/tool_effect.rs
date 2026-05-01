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
#[derive(Debug)]
pub struct ToolError(pub String);

impl std::fmt::Display for ToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for ToolError {}

impl From<crate::CrawlError> for ToolError {
    fn from(value: crate::CrawlError) -> Self {
        Self(value.to_string())
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
        let err = ToolError("something went wrong".to_string());
        assert_eq!(err.to_string(), "something went wrong");
    }

    #[test]
    fn tool_error_from_crawl_error_conversion() {
        let crawl_err = crate::CrawlError::new("crawl failure");
        let tool_err: ToolError = crawl_err.into();
        assert_eq!(tool_err.to_string(), "crawl failure");
    }
}
