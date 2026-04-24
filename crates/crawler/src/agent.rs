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
use crate::state::{ChildBlock, CrawlState};
use crate::tool_registry::ToolRegistry;
use crate::{mvp_tool_specs, AgentManager, BrowserContext, SharedApiClient, SharedBridge};

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
    browser: Option<BrowserContext>,
    registry: ToolRegistry,
    max_steps: usize,
    agent_id: String,
    agent_manager: SharedAgentManager,
    shared_bridge: Option<SharedBridge>,
    crawl_state: CrawlState,
    pub(crate) child_tasks: ChildTaskMap,
    api_client_arc: Option<SharedApiClient>,
    #[cfg(test)]
    fork_page_index_override: Option<usize>,
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

    /// Drop the current browser context so the next tool call will spawn a
    /// fresh Playwright bridge.  This is used by `/headed` and `/headless` to
    /// make the mode switch take effect immediately.
    pub fn reset_browser(&mut self) {
        self.browser = None;
        self.shared_bridge = None;
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

        let mut runtime =
            ConversationRuntime::new(Session::new(), shared_client.clone(), self, system_prompt)
                .with_max_iterations(max_steps);
        let result = runtime.run_turn(goal).await;
        let agent_id = runtime.tool_executor_mut().agent_id.clone();
        let agent_manager = runtime.tool_executor_mut().agent_manager.clone();
        agent_manager.lock().await.mark_done(&agent_id);

        let summary = result.map_err(|e| CrawlError::new(e.to_string()))?;
        let crawl_state = runtime.tool_executor_mut().crawl_state.clone();

        Ok(build_crawl_result(&summary, &crawl_state))
    }

    async fn ensure_browser(&mut self) -> Result<(), ToolError> {
        if self.browser.is_some() {
            return Ok(());
        }

        let shared_bridge = if let Some(shared_bridge) = &self.shared_bridge {
            shared_bridge.clone()
        } else {
            let bridge = crate::PlaywrightBridge::new()
                .await
                .map_err(|error| ToolError::new(error.to_string()))?;
            let shared_bridge = Arc::new(Mutex::new(bridge));
            self.shared_bridge = Some(shared_bridge.clone());
            shared_bridge
        };

        self.browser = Some(BrowserContext::new(shared_bridge));
        Ok(())
    }

    async fn handle_fork(&mut self, input: &str) -> Result<String, ToolError> {
        let params: Value = serde_json::from_str(input)
            .map_err(|error| ToolError::new(format!("fork input must be valid JSON: {error}")))?;

        let sub_goal = params
            .get("sub_goal")
            .and_then(Value::as_str)
            .map_or("", str::trim)
            .to_string();

        if sub_goal.is_empty() {
            return Err(ToolError::new("fork requires non-empty sub_goal"));
        }

        let url = params
            .get("url")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);

        let settings = runtime::load_settings();
        let child_max_steps = runtime::settings_get_fork_child_max_steps(&settings) as usize;
        let child_id = format!("{}-child-{}", self.agent_id, self.child_tasks.len() + 1);

        // Atomic: ensure root registered + claim child slot in one lock scope.
        {
            let mut mgr = self.agent_manager.lock().await;
            if !mgr.contains(&self.agent_id) {
                mgr.max_concurrent_per_parent =
                    runtime::settings_get_max_concurrent_per_parent(&settings) as usize;
                mgr.max_depth = runtime::settings_get_max_fork_depth(&settings) as usize;
                mgr.max_total = runtime::settings_get_max_total_agents(&settings) as usize;
                mgr.register_root(self.agent_id.clone());
            }
            mgr.register_child(child_id.clone(), &self.agent_id, None)
                .map_err(|error| ToolError::new(error.to_string()))?;
        }

        // Expensive setup — if anything below fails, release the claimed slot.
        let setup = async {
            self.ensure_browser().await?;

            let child_state = self
                .crawl_state
                .fork(&sub_goal, url.as_deref(), child_max_steps);
            let child_api_client = self
                .api_client_arc
                .clone()
                .ok_or_else(|| ToolError::new("fork: api_client not initialized"))?;
            let shared_bridge = self
                .shared_bridge
                .clone()
                .ok_or_else(|| ToolError::new("fork: browser bridge not initialized"))?;
            let target_url = url.clone().or_else(|| self.crawl_state.current_url.clone());

            let page_index = self
                .create_child_page(shared_bridge.clone(), target_url.as_deref())
                .await?;

            Ok::<_, ToolError>((child_state, child_api_client, shared_bridge, page_index))
        }
        .await;

        let (child_state, child_api_client, shared_bridge, page_index) = match setup {
            Ok(values) => values,
            Err(e) => {
                self.agent_manager.lock().await.mark_done(&child_id);
                return Err(e);
            }
        };

        let mut child_agent = CrawlerAgent::new(
            BrowserContext::new_shared(shared_bridge.clone(), page_index),
            ToolRegistry::new_with_core_tools(),
        )
        .with_max_steps(child_max_steps)
        .with_agent_id(child_id.clone())
        .with_agent_manager(self.agent_manager.clone());
        child_agent.shared_bridge = Some(shared_bridge);
        child_agent.crawl_state = child_state;
        child_agent.api_client_arc = Some(child_api_client.clone());

        let child_sub_goal = sub_goal.clone();
        let rt_handle = tokio::runtime::Handle::current();
        let join_handle: tokio::task::JoinHandle<Option<Vec<Value>>> =
            tokio::task::spawn_blocking(move || {
                rt_handle
                    .block_on(child_agent.run(&child_sub_goal, child_api_client))
                    .ok()
                    .map(|crawl_result| crawl_result.extracted_data)
            });
        self.child_tasks
            .insert(child_id.clone(), (sub_goal.clone(), join_handle));

        self.crawl_state
            .action_history
            .push(format!("Forked subagent {child_id} for: {sub_goal}"));

        Ok(format!("Forked subagent {child_id} for: {sub_goal}"))
    }

    async fn handle_wait_for_subagents(&mut self) -> Result<String, ToolError> {
        if self.child_tasks.is_empty() {
            return Ok("No active subagents".to_string());
        }

        let settings = runtime::load_settings();
        let wait_timeout_secs = u64::from(runtime::settings_get_fork_wait_timeout_secs(&settings));
        let timeout = std::time::Duration::from_secs(wait_timeout_secs);
        let deadline = tokio::time::Instant::now() + timeout;
        let tasks: Vec<_> = self
            .child_tasks
            .drain()
            .map(|(child_id, (sub_goal, handle))| (child_id, sub_goal, handle))
            .collect();

        let task_count = tasks.len();
        let mut total_items = 0_usize;

        for (child_id, sub_goal, mut handle) in tasks {
            let remaining = deadline
                .checked_duration_since(tokio::time::Instant::now())
                .unwrap_or_default();
            let items = if remaining.is_zero() {
                handle.abort();
                Vec::new()
            } else {
                match tokio::time::timeout(remaining, &mut handle).await {
                    Ok(Ok(Some(items))) => items,
                    Ok(Ok(None) | Err(_)) => Vec::new(),
                    Err(_) => {
                        handle.abort();
                        Vec::new()
                    }
                }
            };

            total_items += items.len();
            self.crawl_state.child_blocks.push(ChildBlock {
                child_id: child_id.clone(),
                sub_goal,
                items,
            });

            self.agent_manager.lock().await.mark_done(&child_id);
        }

        let msg = format!("Waited for {task_count} subagent(s). Collected {total_items} item(s).");
        self.crawl_state.action_history.push(msg.clone());

        Ok(msg)
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

    async fn create_child_page(
        &self,
        shared_bridge: SharedBridge,
        target_url: Option<&str>,
    ) -> Result<usize, ToolError> {
        #[cfg(test)]
        if let Some(page_index) = self.fork_page_index_override {
            return Ok(page_index);
        }

        let mut bridge = shared_bridge.lock().await;
        bridge
            .new_page(target_url)
            .await
            .map_err(|error| ToolError::new(error.to_string()))
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
                .map_err(|e| ToolError::new(format!("invalid JSON input: {e}")))?
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
        .flat_map(|msg| msg.blocks.iter())
        .find_map(|block| {
            if let ContentBlock::Text { text } = block {
                Some(text.clone())
            } else {
                None
            }
        })
        .unwrap_or_default();

    let mut extracted_data = summary
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

    struct CountingTextOnlyApiClient {
        call_count: Arc<std::sync::Mutex<usize>>,
    }

    impl ApiClient for CountingTextOnlyApiClient {
        fn stream(&mut self, _request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
            let mut call_count = self
                .call_count
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *call_count += 1;
            Ok(vec![
                AssistantEvent::TextDelta("Child done.".to_string()),
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

        let result = agent.execute("navigate", "").await;
        assert!(result.is_ok());
        let output = result.unwrap();
        assert!(
            output.contains("unknown"),
            "expected fallback url in mock: {output}"
        );
    }

    #[tokio::test]
    async fn run_executes_agent_loop_and_returns_result() {
        let agent = CrawlerAgent::new_for_testing(mock_registry()).with_max_steps(10);
        let api_client = MockApiClient { call_count: 0 };

        let result = agent
            .run("Navigate to example.com", api_client)
            .await
            .expect("agent run should succeed");

        assert_eq!(result.steps_executed, 2);
        assert_eq!(result.summary, "Found the page content.");
    }

    #[tokio::test]
    async fn run_returns_immediately_when_llm_gives_text_only() {
        let agent = CrawlerAgent::new_for_testing(mock_registry());
        let api_client = TextOnlyApiClient;

        let result = agent
            .run("just a question", api_client)
            .await
            .expect("should succeed");

        assert_eq!(result.steps_executed, 1);
        assert_eq!(result.summary, "All done.");
    }

    #[tokio::test]
    async fn run_collects_extracted_data_from_tool_results() {
        let agent = CrawlerAgent::new_for_testing(mock_registry());
        let api_client = ExtractDataApiClient { call_count: 0 };

        let result = agent
            .run("extract titles from example.com", api_client)
            .await
            .expect("should succeed");

        assert_eq!(result.extracted_data.len(), 1);
        assert_eq!(result.extracted_data[0]["title"], "Example");
    }

    #[tokio::test]
    async fn max_steps_limit_is_enforced() {
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
            .await
            .expect_err("should fail due to max iterations");

        assert!(err.to_string().contains("maximum"));
    }

    #[tokio::test]
    async fn test_fork_dispatch_spawns_child() {
        let manager = default_agent_manager();
        manager.lock().await.register_root("root");

        let call_count = Arc::new(std::sync::Mutex::new(0usize));
        let shared_client = SharedApiClient::new(CountingTextOnlyApiClient {
            call_count: Arc::clone(&call_count),
        });
        let mut agent = CrawlerAgent::new_for_testing(ToolRegistry::new_with_core_tools())
            .with_agent_id("root".to_string())
            .with_agent_manager(manager.clone());
        agent.api_client_arc = Some(shared_client);
        agent.shared_bridge = Some(Arc::new(Mutex::new(
            crate::PlaywrightBridge::new()
                .await
                .expect("bridge should initialize for fork test"),
        )));
        agent.fork_page_index_override = Some(1);

        let observation = agent
            .execute(
                "fork",
                r#"{"sub_goal":"check result","url":"https://example.com"}"#,
            )
            .await
            .expect("fork should succeed");

        assert!(observation.contains("Forked subagent root-child-1 for: check result"));
        assert_eq!(manager.lock().await.get_children("root").len(), 1);
        assert_eq!(agent.child_tasks.len(), 1);
        for (_, (_, handle)) in agent.child_tasks.drain() {
            let _ = handle.await;
        }

        assert_eq!(
            *call_count
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            1
        );
    }

    #[tokio::test]
    async fn test_fork_returns_observation() {
        let manager = default_agent_manager();
        manager.lock().await.register_root("root");

        let mut agent = CrawlerAgent::new_for_testing(ToolRegistry::new_with_core_tools())
            .with_agent_id("root".to_string())
            .with_agent_manager(manager);
        agent.api_client_arc = Some(SharedApiClient::new(TextOnlyApiClient));
        agent.shared_bridge = Some(Arc::new(Mutex::new(
            crate::PlaywrightBridge::new()
                .await
                .expect("bridge should initialize for fork test"),
        )));
        agent.fork_page_index_override = Some(1);

        let observation = agent
            .execute("fork", r#"{"sub_goal":"collect details"}"#)
            .await
            .expect("fork should succeed");

        assert_eq!(
            observation,
            "Forked subagent root-child-1 for: collect details"
        );

        for (_, (_, handle)) in agent.child_tasks.drain() {
            handle.abort();
        }
    }

    #[tokio::test]
    async fn test_fork_at_limit_returns_error() {
        let manager = Arc::new(Mutex::new(AgentManager::new(0, 3, 10)));
        manager.lock().await.register_root("root");

        let mut agent = CrawlerAgent::new_for_testing(ToolRegistry::new_with_core_tools())
            .with_agent_id("root".to_string())
            .with_agent_manager(manager);
        agent.api_client_arc = Some(SharedApiClient::new(TextOnlyApiClient));

        let err = agent
            .execute("fork", r#"{"sub_goal":"blocked"}"#)
            .await
            .expect_err("fork at limit should return an error");

        assert!(
            err.to_string().contains("active children"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn test_fork_empty_subgoal_returns_error() {
        let manager = default_agent_manager();
        manager.lock().await.register_root("root");

        let mut agent = CrawlerAgent::new_for_testing(ToolRegistry::new_with_core_tools())
            .with_agent_id("root".to_string())
            .with_agent_manager(manager);

        let err = agent
            .execute("fork", r#"{"sub_goal":"   "}"#)
            .await
            .expect_err("empty sub_goal should fail");

        assert_eq!(err.to_string(), "fork requires non-empty sub_goal");
    }

    #[tokio::test]
    async fn test_wait_no_children_returns_immediately() {
        let mut agent = CrawlerAgent::new_for_testing(mock_registry());

        let result = agent.handle_wait_for_subagents().await;

        assert_eq!(result.unwrap(), "No active subagents");
    }

    #[tokio::test]
    async fn test_wait_records_step() {
        let manager = default_agent_manager();
        manager.lock().await.register_root("test-agent");

        let mut agent = CrawlerAgent::new_for_testing(mock_registry()).with_agent_manager(manager);
        let handle: tokio::task::JoinHandle<Option<Vec<Value>>> = tokio::spawn(async { None });
        agent
            .child_tasks
            .insert("child-1".to_string(), ("search".to_string(), handle));

        let result = agent.execute("wait_for_subagents", "{}").await.unwrap();

        assert_eq!(result, "Waited for 1 subagent(s). Collected 0 item(s).");
        assert_eq!(agent.crawl_state.action_history.len(), 1);
        assert_eq!(agent.crawl_state.action_history[0], result);
    }

    #[tokio::test]
    async fn test_done_sets_state_done() {
        let mut agent = CrawlerAgent::new_for_testing(mock_registry());

        let result = agent
            .execute("done", r#"{"summary":"Task complete"}"#)
            .await;

        assert_eq!(result.unwrap(), "Task complete");
        assert!(agent.crawl_state.done);
        assert_eq!(agent.crawl_state.done_reason, "Task complete");
    }

    #[tokio::test]
    async fn test_done_auto_waits_for_children() {
        let manager = default_agent_manager();
        manager.lock().await.register_root("test-agent");
        manager
            .lock()
            .await
            .register_child("child-1", "test-agent", None)
            .expect("child registration should succeed");

        let mut agent = CrawlerAgent::new_for_testing(mock_registry()).with_agent_manager(manager);
        let handle = tokio::spawn(async { Some(vec![serde_json::json!({"child": 1})]) });
        agent
            .child_tasks
            .insert("child-1".to_string(), ("goal".to_string(), handle));

        let result = agent
            .execute("done", r#"{"summary":"Finished"}"#)
            .await
            .unwrap();

        assert_eq!(result, "Finished");
        assert!(agent.crawl_state.done);
        assert_eq!(agent.crawl_state.done_reason, "Finished");
        assert!(agent.child_tasks.is_empty());
        assert_eq!(agent.crawl_state.child_blocks.len(), 1);
        assert_eq!(agent.crawl_state.action_history.len(), 1);
        assert_eq!(
            agent.crawl_state.action_history[0],
            "Waited for 1 subagent(s). Collected 1 item(s)."
        );
    }

    #[tokio::test]
    async fn test_merge_child_data_to_parent() {
        let manager = default_agent_manager();
        manager.lock().await.register_root("test-agent");
        manager
            .lock()
            .await
            .register_child("child-1", "test-agent", None)
            .expect("child registration should succeed");

        let mut agent = CrawlerAgent::new_for_testing(mock_registry()).with_agent_manager(manager);
        let handle: tokio::task::JoinHandle<Option<Vec<Value>>> =
            tokio::spawn(async { Some(vec![serde_json::json!({"data": 1})]) });
        agent
            .child_tasks
            .insert("child-1".to_string(), ("search".to_string(), handle));

        let result = agent.execute("wait_for_subagents", "{}").await.unwrap();

        assert!(result.contains("Collected 1"));
        assert_eq!(agent.crawl_state.child_blocks.len(), 1);
        assert_eq!(agent.crawl_state.child_blocks[0].child_id, "child-1");
        assert_eq!(agent.crawl_state.child_blocks[0].sub_goal, "search");
        assert_eq!(agent.crawl_state.child_blocks[0].items.len(), 1);
    }

    #[tokio::test]
    async fn test_merge_preserves_parent_data() {
        let manager = default_agent_manager();
        manager.lock().await.register_root("test-agent");
        manager
            .lock()
            .await
            .register_child("child-1", "test-agent", None)
            .expect("child registration should succeed");

        let mut agent = CrawlerAgent::new_for_testing(mock_registry()).with_agent_manager(manager);
        agent.crawl_state.extracted_data = vec![serde_json::json!({"parent": 1})];

        let handle = tokio::spawn(async { Some(vec![serde_json::json!({"child": 1})]) });
        agent
            .child_tasks
            .insert("child-1".to_string(), ("goal".to_string(), handle));

        let _ = agent.execute("wait_for_subagents", "{}").await;

        let all = agent.crawl_state.all_data();
        assert_eq!(all.len(), 2);
    }

    #[tokio::test]
    async fn test_message_stop_still_works_without_done() {
        let agent = CrawlerAgent::new_for_testing(mock_registry());
        let api_client = TextOnlyApiClient;

        let result = agent.run("test", api_client).await;

        assert!(result.is_ok());
        let result = result.unwrap();
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
        let crawl_state = CrawlState::default();

        let result = build_crawl_result(&summary, &crawl_state);
        assert_eq!(result.summary, "final answer");
        assert_eq!(result.steps_executed, 2);
    }

    #[test]
    fn build_crawl_result_merges_child_data() {
        let summary = TurnSummary {
            assistant_messages: vec![runtime::ConversationMessage::assistant(vec![
                ContentBlock::Text {
                    text: "final answer".to_string(),
                },
            ])],
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
            child_blocks: vec![ChildBlock {
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

    #[tokio::test]
    async fn test_fork_full_lifecycle() {
        let mut parent = CrawlerAgent::new_for_testing(mock_registry());
        parent.crawl_state.extracted_data = vec![serde_json::json!({"parent": 1})];

        // Simulate a completed child task
        let handle: tokio::task::JoinHandle<Option<Vec<Value>>> = tokio::spawn(async {
            Some(vec![
                serde_json::json!({"child": 1}),
                serde_json::json!({"child": 2}),
            ])
        });
        parent
            .child_tasks
            .insert("child-1".to_string(), ("search page 2".to_string(), handle));

        let wait_result = parent.execute("wait_for_subagents", "{}").await.unwrap();
        assert!(wait_result.contains("Collected 2"));

        let all = parent.crawl_state.all_data();
        assert_eq!(all.len(), 3);
        assert_eq!(parent.crawl_state.child_blocks.len(), 1);
        assert_eq!(parent.crawl_state.child_blocks[0].sub_goal, "search page 2");
    }

    #[tokio::test]
    async fn test_fork_immediate_done_auto_waits() {
        let mut parent = CrawlerAgent::new_for_testing(mock_registry());

        let handle: tokio::task::JoinHandle<Option<Vec<Value>>> =
            tokio::spawn(async { Some(vec![serde_json::json!({"found": "data"})]) });
        parent
            .child_tasks
            .insert("child-1".to_string(), ("find data".to_string(), handle));

        let done_result = parent
            .execute("done", r#"{"summary": "all done"}"#)
            .await
            .unwrap();

        assert_eq!(done_result, "all done");
        assert!(parent.crawl_state.done);
        assert_eq!(parent.crawl_state.done_reason, "all done");
        assert_eq!(parent.crawl_state.child_blocks.len(), 1);
        assert_eq!(parent.crawl_state.child_blocks[0].items.len(), 1);
    }

    #[test]
    fn test_fork_depth_2_lifecycle() {
        let mut mgr = AgentManager::new(10, 2, 20);
        mgr.register_root("root");
        mgr.register_child("child1", "root", None).unwrap();
        mgr.register_child("grandchild1", "child1", None).unwrap();

        assert_eq!(mgr.get_depth("root"), 0);
        assert_eq!(mgr.get_depth("child1"), 1);
        assert_eq!(mgr.get_depth("grandchild1"), 2);
        assert!(!mgr.can_fork("grandchild1")); // depth 2, max_depth=2, child would be depth 3
    }
}
