use std::collections::HashMap;
use std::fmt::{Display, Formatter};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use runtime::{
    ApiClient, ContentBlock, ConversationRuntime, Session, ToolError, ToolExecutor, TurnSummary,
};
use serde_json::Value;
use tokio::sync::Mutex;

use crate::manager::SharedAgentManager;
use crate::prompt::build_system_prompt;
use crate::state::CrawlState;
use crate::tool_registry::ToolRegistry;
use crate::{mvp_tool_specs, AgentManager, BrowserContext, SharedApiClient, SharedBridge};

mod fork;
mod lifecycle;

#[allow(unused_imports)]
pub(crate) use fork::*;
#[allow(unused_imports)]
pub(crate) use lifecycle::*;

const DEFAULT_MAX_STEPS: usize = 50;
const DEFAULT_MAX_CONCURRENT_PER_PARENT: usize = 5;
const DEFAULT_MAX_FORK_DEPTH: usize = 3;
const DEFAULT_MAX_TOTAL_AGENTS: usize = 10;

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

type ChildTaskMap = HashMap<String, (String, tokio::task::JoinHandle<Option<Vec<Value>>>)>;

pub struct CrawlerAgent {
    pub(super) browser: Option<BrowserContext>,
    registry: ToolRegistry,
    max_steps: usize,
    pub(super) agent_id: String,
    pub(super) agent_manager: SharedAgentManager,
    pub(super) shared_bridge: Option<SharedBridge>,
    pub(super) crawl_state: CrawlState,
    pub(crate) child_tasks: ChildTaskMap,
    pub(super) api_client_arc: Option<SharedApiClient>,
    #[cfg(test)]
    pub(super) fork_page_index_override: Option<usize>,
}

impl CrawlerAgent {
    #[must_use]
    pub fn new(browser: BrowserContext, registry: ToolRegistry) -> Self {
        Self {
            shared_bridge: Some(browser.bridge().clone()),
            browser: Some(browser),
            registry,
            max_steps: DEFAULT_MAX_STEPS,
            agent_id: generate_agent_id(),
            agent_manager: default_agent_manager(),
            crawl_state: CrawlState {
                max_steps: DEFAULT_MAX_STEPS,
                ..CrawlState::default()
            },
            child_tasks: HashMap::new(),
            api_client_arc: None,
            #[cfg(test)]
            fork_page_index_override: None,
        }
    }

    #[must_use]
    pub fn new_lazy(registry: ToolRegistry) -> Self {
        Self {
            browser: None,
            registry,
            max_steps: DEFAULT_MAX_STEPS,
            agent_id: generate_agent_id(),
            agent_manager: default_agent_manager(),
            shared_bridge: None,
            crawl_state: CrawlState {
                max_steps: DEFAULT_MAX_STEPS,
                ..CrawlState::default()
            },
            child_tasks: HashMap::new(),
            api_client_arc: None,
            #[cfg(test)]
            fork_page_index_override: None,
        }
    }

    #[cfg(test)]
    fn new_for_testing(registry: ToolRegistry) -> Self {
        Self {
            browser: None,
            registry,
            max_steps: DEFAULT_MAX_STEPS,
            agent_id: "test-agent".to_string(),
            agent_manager: default_agent_manager(),
            shared_bridge: None,
            crawl_state: CrawlState {
                max_steps: DEFAULT_MAX_STEPS,
                ..CrawlState::default()
            },
            child_tasks: HashMap::new(),
            api_client_arc: None,
            #[cfg(test)]
            fork_page_index_override: None,
        }
    }

    #[must_use]
    pub fn with_max_steps(mut self, max_steps: usize) -> Self {
        self.max_steps = max_steps;
        self.crawl_state.max_steps = max_steps;
        self
    }

    #[must_use]
    pub fn with_agent_id(mut self, id: String) -> Self {
        self.agent_id = id;
        self
    }

    #[must_use]
    pub fn with_agent_manager(mut self, manager: SharedAgentManager) -> Self {
        self.agent_manager = manager;
        self
    }

    #[must_use]
    pub fn with_api_client(mut self, client: SharedApiClient) -> Self {
        self.api_client_arc = Some(client);
        self
    }

    pub async fn run(
        mut self,
        goal: &str,
        api_client: impl ApiClient + Send + Sync + 'static,
    ) -> Result<CrawlResult, CrawlError> {
        let shared_client = SharedApiClient::new(api_client);
        self.api_client_arc = Some(shared_client.clone());

        let settings = runtime::load_settings();
        {
            let mut manager = self.agent_manager.lock().await;
            if !manager.contains(&self.agent_id) {
                manager.max_concurrent_per_parent =
                    runtime::settings_get_max_concurrent_per_parent(&settings) as usize;
                manager.max_depth = runtime::settings_get_max_fork_depth(&settings) as usize;
                manager.max_total = runtime::settings_get_max_total_agents(&settings) as usize;
                manager.register_root(self.agent_id.clone());
            }
        }

        let max_steps = self.max_steps;
        let system_prompt = build_system_prompt(&mvp_tool_specs());
        let mut runtime = ConversationRuntime::new(Session::new(), shared_client.clone(), self, system_prompt)
            .with_max_iterations(max_steps);
        let result = runtime.run_turn(goal).await;
        let agent_id = runtime.tool_executor_mut().agent_id.clone();
        let agent_manager = runtime.tool_executor_mut().agent_manager.clone();
        agent_manager.lock().await.mark_done(&agent_id);

        let summary = result.map_err(|error| CrawlError::new(error.to_string()))?;
        let crawl_state = runtime.tool_executor_mut().crawl_state.clone();
        Ok(build_crawl_result(&summary, &crawl_state))
    }

    async fn handle_done(&mut self, input: &str) -> Result<String, ToolError> {
        let params: Value = serde_json::from_str(input)
            .map_err(|error| ToolError::new(format!("done input must be valid JSON: {error}")))?;

        let summary = params
            .get("summary")
            .and_then(Value::as_str)
            .unwrap_or("Task complete")
            .to_string();

        if !self.child_tasks.is_empty() {
            let _ = self.handle_wait_for_subagents().await;
        }

        self.crawl_state.done = true;
        self.crawl_state.done_reason.clone_from(&summary);
        Ok(summary)
    }

    fn supports_async(tool_name: &str) -> bool {
        matches!(
            tool_name,
            "navigate"
                | "click"
                | "fill_form"
                | "extract_data"
                | "screenshot"
                | "go_back"
                | "scroll"
                | "wait"
                | "select_option"
                | "execute_js"
                | "hover"
                | "press_key"
                | "switch_tab"
                | "list_resources"
                | "save_file"
        )
    }
}

impl Drop for CrawlerAgent {
    fn drop(&mut self) {
        for (_, (_, handle)) in self.child_tasks.drain() {
            handle.abort();
        }
    }
}

impl ToolExecutor for CrawlerAgent {
    async fn execute(&mut self, tool_name: &str, input: &str) -> Result<String, ToolError> {
        if tool_name == "fork" {
            return self.handle_fork(input).await;
        }

        if tool_name == "wait_for_subagents" {
            return self.handle_wait_for_subagents().await;
        }

        if tool_name == "done" {
            return self.handle_done(input).await;
        }

        let input_value: Value = if input.is_empty() {
            Value::Object(serde_json::Map::new())
        } else {
            serde_json::from_str(input)
                .map_err(|error| ToolError::new(format!("invalid JSON input: {error}")))?
        };

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
            self.ensure_browser().await?;
            let browser = self
                .browser
                .as_mut()
                .ok_or_else(|| ToolError::new("browser context is not initialized"))?;
            let result = self
                .registry
                .execute_async(tool_name, &input_value, browser)
                .await
                .map_err(|error| ToolError::new(error.to_string()))?;
            return Ok(result.to_string());
        }

        Err(ToolError::new(format!("unknown tool: `{tool_name}`")))
    }
}

fn default_agent_manager() -> SharedAgentManager {
    Arc::new(Mutex::new(AgentManager::new(
        DEFAULT_MAX_CONCURRENT_PER_PARENT,
        DEFAULT_MAX_FORK_DEPTH,
        DEFAULT_MAX_TOTAL_AGENTS,
    )))
}

fn generate_agent_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    format!("agent-{nanos}")
}

fn build_crawl_result(summary: &TurnSummary, crawl_state: &CrawlState) -> CrawlResult {
    let text_summary = summary
        .assistant_messages
        .iter()
        .rev()
        .flat_map(|message| message.blocks.iter())
        .find_map(|block| match block {
            ContentBlock::Text { text } => Some(text.clone()),
            _ => None,
        })
        .unwrap_or_default();

    let mut extracted_data = summary
        .tool_results
        .iter()
        .flat_map(|message| message.blocks.iter())
        .filter_map(|block| match block {
            ContentBlock::ToolResult {
                tool_name,
                output,
                is_error: false,
                ..
            } if tool_name == "extract_data" => serde_json::from_str(output).ok(),
            _ => None,
        })
        .collect::<Vec<_>>();

    extracted_data.extend(crawl_state.all_data());

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
                    .and_then(Value::as_str)
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

    #[tokio::test]
    async fn crawler_agent_dispatches_tool_through_registry() {
        let mut agent = CrawlerAgent::new_for_testing(mock_registry());
        let result = agent
            .execute("navigate", r#"{"url":"https://example.com"}"#)
            .await
            .expect("navigate should succeed");
        assert!(result.contains("Navigated to https://example.com"));
    }

    #[tokio::test]
    async fn crawler_agent_returns_error_for_unknown_tool() {
        let mut agent = CrawlerAgent::new_for_testing(mock_registry());
        let err = agent
            .execute("nonexistent", "{}")
            .await
            .expect_err("should fail for unknown tool");
        assert!(err.to_string().contains("unknown tool"));
    }

    #[tokio::test]
    async fn crawler_agent_returns_error_for_invalid_json() {
        let mut agent = CrawlerAgent::new_for_testing(mock_registry());
        let err = agent
            .execute("navigate", "not-json")
            .await
            .expect_err("should fail for invalid JSON");
        assert!(err.to_string().contains("invalid JSON"));
    }

    #[tokio::test]
    async fn crawler_agent_handles_empty_input_as_empty_object() {
        let mut agent = CrawlerAgent::new_for_testing(mock_registry());
        let output = agent
            .execute("navigate", "")
            .await
            .expect("empty input should map to empty object");
        assert!(output.contains("unknown"));
    }

    #[tokio::test]
    async fn run_executes_agent_loop_and_returns_result() {
        let agent = CrawlerAgent::new_for_testing(mock_registry()).with_max_steps(10);
        let result = agent
            .run("Navigate to example.com", MockApiClient { call_count: 0 })
            .await
            .expect("agent run should succeed");
        assert_eq!(result.steps_executed, 2);
        assert_eq!(result.summary, "Found the page content.");
    }

    #[tokio::test]
    async fn run_collects_extracted_data_from_tool_results() {
        let agent = CrawlerAgent::new_for_testing(mock_registry());
        let result = agent
            .run("extract titles from example.com", ExtractDataApiClient { call_count: 0 })
            .await
            .expect("should succeed");
        assert_eq!(result.extracted_data.len(), 1);
        assert_eq!(result.extracted_data[0]["title"], "Example");
    }

    #[tokio::test]
    async fn run_returns_immediately_when_llm_gives_text_only() {
        let result = CrawlerAgent::new_for_testing(mock_registry())
            .run("just a question", TextOnlyApiClient)
            .await
            .expect("should succeed");
        assert_eq!(result.steps_executed, 1);
        assert_eq!(result.summary, "All done.");
    }

    #[tokio::test]
    async fn max_steps_limit_is_enforced() {
        struct InfiniteLoopApiClient;

        impl ApiClient for InfiniteLoopApiClient {
            fn stream(&mut self, _request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
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

        let err = CrawlerAgent::new_for_testing(mock_registry())
            .with_max_steps(3)
            .run("loop forever", InfiniteLoopApiClient)
            .await
            .expect_err("should fail due to max iterations");
        assert!(err.to_string().contains("maximum"));
    }

    #[tokio::test]
    async fn test_message_stop_still_works_without_done() {
        let result = CrawlerAgent::new_for_testing(mock_registry())
            .run("test", TextOnlyApiClient)
            .await
            .expect("text-only run should succeed");
        assert_eq!(result.steps_executed, 1);
        assert_eq!(result.summary, "All done.");
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

        let result = build_crawl_result(&summary, &CrawlState::default());
        assert_eq!(result.summary, "final answer");
        assert_eq!(result.steps_executed, 2);
    }

    #[test]
    fn build_crawl_result_merges_child_data() {
        let summary = TurnSummary {
            assistant_messages: vec![runtime::ConversationMessage::assistant(vec![ContentBlock::Text {
                text: "final answer".to_string(),
            }])],
            tool_results: vec![runtime::ConversationMessage::tool_result(
                "call-1".to_string(),
                "extract_data".to_string(),
                r#"{"parent":1}"#.to_string(),
                false,
            )],
            iterations: 1,
            usage: TokenUsage {
                input_tokens: 0,
                output_tokens: 0,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: 0,
            },
            auto_compaction: None,
        };
        let crawl_state = CrawlState {
            child_blocks: vec![crate::state::ChildBlock {
                child_id: "child-1".to_string(),
                sub_goal: "goal".to_string(),
                items: vec![serde_json::json!({"child": 1})],
            }],
            ..CrawlState::default()
        };

        let result = build_crawl_result(&summary, &crawl_state);
        assert_eq!(result.extracted_data.len(), 2);
        assert_eq!(result.extracted_data[0], serde_json::json!({"parent": 1}));
        assert_eq!(result.extracted_data[1], serde_json::json!({"child": 1}));
    }
}
