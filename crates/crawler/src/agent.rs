use std::fmt::{Display, Formatter};

use runtime::{
    ApiClient, ContentBlock, ConversationRuntime, PermissionMode, PermissionPolicy, Session,
    ToolError, ToolExecutor, TurnSummary,
};
use serde_json::Value;

use crate::prompt::build_system_prompt;
use crate::state::CrawlState;
use crate::tool_registry::ToolRegistry;
use crate::{mvp_tool_specs, BrowserContext};

const DEFAULT_MAX_STEPS: usize = 50;

#[derive(Debug, Clone, Default)]
pub struct AgentState {
    pub goal: String,
    pub max_steps: usize,
    pub crawl_state: CrawlState,
}

#[derive(Debug, Clone, Default)]
pub struct CrawlResult {
    pub summary: String,
    pub extracted_data: Vec<Value>,
    pub steps_executed: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentHandle {
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrawlError {
    message: String,
}

impl CrawlError {
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl Display for CrawlError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for CrawlError {}

#[allow(async_fn_in_trait)]
pub trait CrawlAgent {
    async fn run(&mut self, goal: &str) -> Result<CrawlResult, CrawlError>;

    fn fork(&mut self) -> Result<AgentHandle, CrawlError> {
        unimplemented!("forking is not implemented yet; this is a design stub")
    }
}

pub struct CrawlerAgent {
    browser: Option<BrowserContext>,
    registry: ToolRegistry,
    max_steps: usize,
}

impl CrawlerAgent {
    #[must_use]
    pub fn new(browser: BrowserContext, registry: ToolRegistry) -> Self {
        Self {
            browser: Some(browser),
            registry,
            max_steps: DEFAULT_MAX_STEPS,
        }
    }

    #[must_use]
    pub fn new_lazy(registry: ToolRegistry) -> Self {
        Self {
            browser: None,
            registry,
            max_steps: DEFAULT_MAX_STEPS,
        }
    }

    #[cfg(test)]
    fn new_for_testing(registry: ToolRegistry) -> Self {
        Self {
            browser: None,
            registry,
            max_steps: DEFAULT_MAX_STEPS,
        }
    }

    #[must_use]
    pub fn with_max_steps(mut self, max_steps: usize) -> Self {
        self.max_steps = max_steps;
        self
    }

    /// Drop the current browser context so the next tool call will spawn a
    /// fresh Playwright bridge.  This is used by `/headed` and `/headless` to
    /// make the mode switch take effect immediately.
    pub fn reset_browser(&mut self) {
        self.browser = None;
    }

    pub fn run(self, goal: &str, api_client: impl ApiClient) -> Result<CrawlResult, CrawlError> {
        let max_steps = self.max_steps;
        let system_prompt = build_system_prompt(&mvp_tool_specs());

        let mut runtime = ConversationRuntime::new(
            Session::new(),
            api_client,
            self,
            PermissionPolicy::new(PermissionMode::DangerFullAccess),
            system_prompt,
        )
        .with_max_iterations(max_steps);

        let summary = runtime
            .run_turn(goal, None)
            .map_err(|e| CrawlError::new(e.to_string()))?;

        Ok(build_crawl_result(&summary))
    }

    fn ensure_browser(&mut self) -> Result<(), ToolError> {
        if self.browser.is_some() {
            return Ok(());
        }

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| ToolError::new(format!("failed to create async runtime: {error}")))?;
        let bridge = runtime
            .block_on(crate::PlaywrightBridge::new())
            .map_err(|error| ToolError::new(error.to_string()))?;
        self.browser = Some(BrowserContext::new(bridge));
        Ok(())
    }

    fn supports_async(tool_name: &str) -> bool {
        matches!(
            tool_name,
            "navigate" | "click" | "fill_form" | "extract_data" | "screenshot" | "go_back"
        )
    }
}

impl ToolExecutor for CrawlerAgent {
    fn execute(&mut self, tool_name: &str, input: &str) -> Result<String, ToolError> {
        let input_value: Value = serde_json::from_str(input)
            .map_err(|e| ToolError::new(format!("invalid JSON input: {e}")))?;

        if let Some(handler) = self.registry.get(tool_name) {
            match handler(&input_value) {
                Ok(result) => return Ok(result.to_string()),
                Err(error)
                    if error
                        .to_string()
                        .contains("requires async execution via execute_async") => {}
                Err(error) => return Err(ToolError::new(error.to_string())),
            }
        }

        if Self::supports_async(tool_name) {
            self.ensure_browser()?;
            let browser = self
                .browser
                .as_mut()
                .ok_or_else(|| ToolError::new("browser context is not initialized"))?;
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|error| {
                    ToolError::new(format!("failed to create async runtime: {error}"))
                })?;
            let result = runtime
                .block_on(
                    self.registry
                        .execute_async(tool_name, &input_value, browser),
                )
                .map_err(|error| ToolError::new(error.to_string()))?;
            return Ok(result.to_string());
        }

        Err(ToolError::new(format!("unknown tool: `{tool_name}`")))
    }
}

fn build_crawl_result(summary: &TurnSummary) -> CrawlResult {
    let text_summary = summary
        .assistant_messages
        .iter()
        .rev()
        .flat_map(|msg| msg.blocks.iter())
        .find_map(|block| {
            if let ContentBlock::Text { text } = block {
                Some(text.clone())
            } else {
                None
            }
        })
        .unwrap_or_default();

    let extracted_data = summary
        .tool_results
        .iter()
        .flat_map(|msg| msg.blocks.iter())
        .filter_map(|block| {
            if let ContentBlock::ToolResult {
                tool_name,
                output,
                is_error: false,
                ..
            } = block
            {
                if tool_name == "extract_data" {
                    serde_json::from_str(output).ok()
                } else {
                    None
                }
            } else {
                None
            }
        })
        .collect();

    CrawlResult {
        summary: text_summary,
        extracted_data,
        steps_executed: summary.iterations,
    }
}

#[cfg(test)]
mod tests {
    use runtime::{ApiRequest, AssistantEvent, RuntimeError, TokenUsage};

    use super::*;
    use crate::tool_registry::ToolRegistry;

    struct MockApiClient {
        call_count: usize,
    }

    impl ApiClient for MockApiClient {
        fn stream(&mut self, _request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
            self.call_count += 1;
            match self.call_count {
                1 => Ok(vec![
                    AssistantEvent::ToolUse {
                        id: "call-1".to_string(),
                        name: "navigate".to_string(),
                        input: r#"{"url":"https://example.com"}"#.to_string(),
                    },
                    AssistantEvent::Usage(TokenUsage {
                        input_tokens: 100,
                        output_tokens: 20,
                        cache_creation_input_tokens: 0,
                        cache_read_input_tokens: 0,
                    }),
                    AssistantEvent::MessageStop,
                ]),
                2 => Ok(vec![
                    AssistantEvent::TextDelta("Found the page content.".to_string()),
                    AssistantEvent::Usage(TokenUsage {
                        input_tokens: 120,
                        output_tokens: 10,
                        cache_creation_input_tokens: 0,
                        cache_read_input_tokens: 0,
                    }),
                    AssistantEvent::MessageStop,
                ]),
                _ => Err(RuntimeError::new("unexpected extra API call")),
            }
        }
    }

    struct TextOnlyApiClient;

    impl ApiClient for TextOnlyApiClient {
        fn stream(&mut self, _request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
            Ok(vec![
                AssistantEvent::TextDelta("All done.".to_string()),
                AssistantEvent::MessageStop,
            ])
        }
    }

    struct ExtractDataApiClient {
        call_count: usize,
    }

    impl ApiClient for ExtractDataApiClient {
        fn stream(&mut self, _request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
            self.call_count += 1;
            match self.call_count {
                1 => Ok(vec![
                    AssistantEvent::ToolUse {
                        id: "call-1".to_string(),
                        name: "extract_data".to_string(),
                        input: r#"{"instruction":"get titles","data":{}}"#.to_string(),
                    },
                    AssistantEvent::MessageStop,
                ]),
                2 => Ok(vec![
                    AssistantEvent::TextDelta("Extracted data.".to_string()),
                    AssistantEvent::MessageStop,
                ]),
                _ => Err(RuntimeError::new("unexpected extra API call")),
            }
        }
    }

    fn mock_registry() -> ToolRegistry {
        let mut registry = ToolRegistry::new();
        registry.register(
            "navigate",
            Box::new(|input| {
                let url = input
                    .get("url")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                Ok(Value::String(format!("Navigated to {url}")))
            }),
        );
        registry.register(
            "extract_data",
            Box::new(|input| {
                let data = input.get("data").cloned().unwrap_or(Value::Null);
                Ok(serde_json::json!({"title": "Example", "extracted": data}))
            }),
        );
        registry
    }

    #[test]
    fn crawler_agent_dispatches_tool_through_registry() {
        let mut agent = CrawlerAgent::new_for_testing(mock_registry());

        let result = agent
            .execute("navigate", r#"{"url":"https://example.com"}"#)
            .expect("navigate should succeed");

        assert!(result.contains("Navigated to https://example.com"));
    }

    #[test]
    fn crawler_agent_returns_error_for_unknown_tool() {
        let mut agent = CrawlerAgent::new_for_testing(mock_registry());

        let err = agent
            .execute("nonexistent", "{}")
            .expect_err("should fail for unknown tool");

        assert!(err.to_string().contains("unknown tool"));
    }

    #[test]
    fn crawler_agent_returns_error_for_invalid_json() {
        let mut agent = CrawlerAgent::new_for_testing(mock_registry());

        let err = agent
            .execute("navigate", "not-json")
            .expect_err("should fail for invalid JSON");

        assert!(err.to_string().contains("invalid JSON"));
    }

    #[test]
    fn run_executes_agent_loop_and_returns_result() {
        let agent = CrawlerAgent::new_for_testing(mock_registry()).with_max_steps(10);
        let api_client = MockApiClient { call_count: 0 };

        let result = agent
            .run("Navigate to example.com", api_client)
            .expect("agent run should succeed");

        assert_eq!(result.steps_executed, 2);
        assert_eq!(result.summary, "Found the page content.");
    }

    #[test]
    fn run_returns_immediately_when_llm_gives_text_only() {
        let agent = CrawlerAgent::new_for_testing(mock_registry());
        let api_client = TextOnlyApiClient;

        let result = agent
            .run("just a question", api_client)
            .expect("should succeed");

        assert_eq!(result.steps_executed, 1);
        assert_eq!(result.summary, "All done.");
    }

    #[test]
    fn run_collects_extracted_data_from_tool_results() {
        let agent = CrawlerAgent::new_for_testing(mock_registry());
        let api_client = ExtractDataApiClient { call_count: 0 };

        let result = agent
            .run("extract titles from example.com", api_client)
            .expect("should succeed");

        assert_eq!(result.extracted_data.len(), 1);
        assert_eq!(result.extracted_data[0]["title"], "Example");
    }

    #[test]
    fn max_steps_limit_is_enforced() {
        struct InfiniteLoopApiClient;

        impl ApiClient for InfiniteLoopApiClient {
            fn stream(
                &mut self,
                _request: ApiRequest,
            ) -> Result<Vec<AssistantEvent>, RuntimeError> {
                Ok(vec![
                    AssistantEvent::ToolUse {
                        id: "call-loop".to_string(),
                        name: "navigate".to_string(),
                        input: r#"{"url":"https://example.com"}"#.to_string(),
                    },
                    AssistantEvent::MessageStop,
                ])
            }
        }

        let agent = CrawlerAgent::new_for_testing(mock_registry()).with_max_steps(3);

        let err = agent
            .run("loop forever", InfiniteLoopApiClient)
            .expect_err("should fail due to max iterations");

        assert!(err.to_string().contains("maximum"));
    }

    #[test]
    fn build_crawl_result_extracts_last_assistant_text() {
        let summary = TurnSummary {
            assistant_messages: vec![
                runtime::ConversationMessage::assistant(vec![ContentBlock::Text {
                    text: "first".to_string(),
                }]),
                runtime::ConversationMessage::assistant(vec![ContentBlock::Text {
                    text: "final answer".to_string(),
                }]),
            ],
            tool_results: vec![],
            iterations: 2,
            usage: TokenUsage {
                input_tokens: 0,
                output_tokens: 0,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: 0,
            },
            auto_compaction: None,
        };

        let result = build_crawl_result(&summary);
        assert_eq!(result.summary, "final answer");
        assert_eq!(result.steps_executed, 2);
    }
}
