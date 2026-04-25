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
    /// Tool signals the agent loop should terminate.
    Finish(FinishSpec),
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

/// Parameters for finishing the agent.
#[derive(Debug, Clone)]
pub struct FinishSpec {
    pub summary: String,
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
    fn test_tool_effect_reply_construction() {
        let effect = ToolEffect::Reply("hello".to_string());

        match effect {
            ToolEffect::Reply(reply) => assert_eq!(reply, "hello"),
            _ => panic!("expected reply effect"),
        }
    }

    #[test]
    fn test_fork_spec_fields() {
        let spec = ForkSpec {
            goal: "visit detail page".to_string(),
            page_index: Some(2),
        };

        assert_eq!(spec.goal, "visit detail page");
        assert_eq!(spec.page_index, Some(2));
    }

    #[test]
    fn test_reply_json_serializes_complex_value() {
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
    fn test_tool_error_display_message() {
        let err = ToolError("something went wrong".to_string());
        assert_eq!(err.to_string(), "something went wrong");
        assert!(err.to_string().contains("wrong"));
    }

    #[test]
    fn test_tool_error_from_crawl_error_conversion() {
        let crawl_err = crate::CrawlError::new("crawl failure");
        let tool_err: ToolError = crawl_err.into();
        assert_eq!(tool_err.to_string(), "crawl failure");
    }

    #[test]
    fn test_finish_spec_stores_summary() {
        let spec = FinishSpec {
            summary: "All pages scraped".to_string(),
        };
        assert_eq!(spec.summary, "All pages scraped");
        let cloned = spec.clone();
        assert_eq!(cloned.summary, spec.summary);
    }
}
