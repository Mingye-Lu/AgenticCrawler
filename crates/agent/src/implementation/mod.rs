use std::collections::{BTreeSet, HashMap};
use std::fmt::{Display, Formatter};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use acrawl_core::{ApiClient, ContentBlock, ToolError, ToolExecutor, ToolOutcome};
use runtime::{ControlState, ConversationRuntime, Session, TurnSummary};
use serde_json::Value;
use tokio::sync::Mutex;

use crate::manager::SharedAgentManager;
use crate::prompt::build_system_prompt;
use crate::registry::ToolRegistry;
use crate::state::CrawlState;
use crate::tool_effect::ToolEffect;
use crate::{mvp_tool_specs, AgentManager, BrowserContext, SharedApiClient, SharedBridge};

mod fork;
mod lifecycle;

#[cfg(test)]
use crate::BrowserBackend;

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

impl From<CrawlError> for acrawl_core::error::ToolExecutionError {
    fn from(value: CrawlError) -> Self {
        Self::new(value.to_string())
    }
}

#[allow(async_fn_in_trait)]
pub trait CrawlAgent {
    async fn run(&mut self, goal: &str) -> Result<CrawlResult, CrawlError>;

    fn fork(&mut self) -> Result<AgentHandle, CrawlError> {
        unimplemented!("forking is not implemented yet; this is a design stub")
    }
}

/// Per-child task slot: sub-goal text, the spawned task handle, and the
/// URL [`crate::ClaimGuard`] (dropping the guard releases the claim so a
/// sibling can re-use the scope).
pub(crate) type ChildTaskEntry = (
    String,
    tokio::task::JoinHandle<Option<Vec<Value>>>,
    Option<crate::ClaimGuard>,
);
type ChildTaskMap = HashMap<String, ChildTaskEntry>;

pub struct CrawlerAgent {
    pub(super) browser: Option<BrowserContext>,
    registry: ToolRegistry,
    allowed_tools: Option<BTreeSet<String>>,
    max_steps: usize,
    pub(super) agent_id: String,
    pub(super) agent_manager: SharedAgentManager,
    pub(super) shared_bridge: Option<SharedBridge>,
    pub(super) crawl_state: CrawlState,
    pub(crate) child_tasks: ChildTaskMap,
    /// Monotonically increasing counter feeding `next_child_id`. Using a
    /// per-agent counter (rather than `child_tasks.len()`) prevents IDs from
    /// being reused after children are drained/waited on.
    pub(super) child_id_counter: std::sync::atomic::AtomicU64,
    pub(super) api_client_arc: Option<SharedApiClient>,
    control_state: Option<Arc<ControlState>>,
    child_event_tx: Option<std::sync::mpsc::Sender<crate::child_events::ChildEvent>>,
    child_control_registry: Option<crate::child_events::ChildControlRegistry>,
    extension_mode: bool,
    pub(super) child_snapshots: crate::child_events::ChildSnapshotRegistry,
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
            allowed_tools: None,
            max_steps: DEFAULT_MAX_STEPS,
            agent_id: generate_agent_id(),
            agent_manager: default_agent_manager(),
            crawl_state: CrawlState {
                max_steps: DEFAULT_MAX_STEPS,
                ..CrawlState::default()
            },
            child_tasks: HashMap::new(),
            child_id_counter: std::sync::atomic::AtomicU64::new(0),
            api_client_arc: None,
            control_state: None,
            child_event_tx: None,
            child_control_registry: None,
            extension_mode: false,
            child_snapshots: crate::child_events::ChildSnapshotRegistry::default(),
            #[cfg(test)]
            fork_page_index_override: None,
        }
    }

    #[must_use]
    pub fn new_lazy(registry: ToolRegistry) -> Self {
        Self {
            browser: None,
            registry,
            allowed_tools: None,
            max_steps: DEFAULT_MAX_STEPS,
            agent_id: generate_agent_id(),
            agent_manager: default_agent_manager(),
            shared_bridge: None,
            crawl_state: CrawlState {
                max_steps: DEFAULT_MAX_STEPS,
                ..CrawlState::default()
            },
            child_tasks: HashMap::new(),
            child_id_counter: std::sync::atomic::AtomicU64::new(0),
            api_client_arc: None,
            control_state: None,
            child_event_tx: None,
            child_control_registry: None,
            extension_mode: false,
            child_snapshots: crate::child_events::ChildSnapshotRegistry::default(),
            #[cfg(test)]
            fork_page_index_override: None,
        }
    }

    #[cfg(test)]
    fn new_for_testing(registry: ToolRegistry) -> Self {
        Self {
            browser: None,
            registry,
            allowed_tools: None,
            max_steps: DEFAULT_MAX_STEPS,
            agent_id: "test-agent".to_string(),
            agent_manager: default_agent_manager(),
            shared_bridge: None,
            crawl_state: CrawlState {
                max_steps: DEFAULT_MAX_STEPS,
                ..CrawlState::default()
            },
            child_tasks: HashMap::new(),
            child_id_counter: std::sync::atomic::AtomicU64::new(0),
            api_client_arc: None,
            control_state: None,
            child_event_tx: None,
            child_control_registry: None,
            extension_mode: false,
            child_snapshots: crate::child_events::ChildSnapshotRegistry::default(),
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
    pub fn with_allowed_tools(mut self, allowed_tools: BTreeSet<String>) -> Self {
        self.allowed_tools = Some(allowed_tools);
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

    #[must_use]
    pub fn with_control_state(mut self, state: Arc<ControlState>) -> Self {
        self.control_state = Some(state);
        self
    }

    #[must_use]
    pub fn with_child_event_sender(
        mut self,
        tx: std::sync::mpsc::Sender<crate::child_events::ChildEvent>,
    ) -> Self {
        self.child_event_tx = Some(tx);
        self
    }

    #[must_use]
    pub fn with_child_control_registry(
        mut self,
        registry: crate::child_events::ChildControlRegistry,
    ) -> Self {
        self.child_control_registry = Some(registry);
        self
    }

    #[must_use]
    pub fn with_child_snapshots(
        mut self,
        snapshots: crate::child_events::ChildSnapshotRegistry,
    ) -> Self {
        self.child_snapshots = snapshots;
        self
    }

    pub fn set_control_state(&mut self, state: Arc<ControlState>) {
        self.control_state = Some(state);
    }

    #[must_use]
    pub fn crawl_state(&self) -> &CrawlState {
        &self.crawl_state
    }

    pub async fn run(
        self,
        goal: &str,
        api_client: impl ApiClient + Send + Sync + 'static,
    ) -> Result<CrawlResult, CrawlError> {
        let specs = match &self.allowed_tools {
            Some(allowed) => mvp_tool_specs()
                .into_iter()
                .filter(|s| allowed.contains(s.name))
                .collect(),
            None => mvp_tool_specs(),
        };
        let system_prompt = build_system_prompt(&specs);
        self.run_with_system_prompt(goal, api_client, system_prompt)
            .await
    }

    pub async fn run_with_system_prompt(
        mut self,
        goal: &str,
        api_client: impl ApiClient + Send + Sync + 'static,
        system_prompt: Vec<String>,
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
        let mut runtime =
            ConversationRuntime::new(Session::new(), shared_client.clone(), self, system_prompt)
                .with_max_iterations(max_steps);

        // Attach child event observer for streaming child output to TUI and
        // mirroring lifecycle/heartbeat data into the shared snapshot registry.
        if let Some(tx) = runtime.tool_executor_mut().child_event_tx.take() {
            let snapshots = runtime.tool_executor_mut().child_snapshots.clone();
            let observer = crate::child_events::ChildEventSender::new(
                runtime.tool_executor_mut().agent_id.clone(),
                goal.to_string(),
                tx,
                max_steps,
            )
            .with_snapshots(snapshots);
            runtime = runtime.with_observer(Box::new(observer));
        }

        let control_state = runtime.control_state();
        runtime.tool_executor_mut().set_control_state(control_state);
        let result = runtime.run_turn(goal).await;
        let agent_id = runtime.tool_executor_mut().agent_id.clone();
        let agent_manager = runtime.tool_executor_mut().agent_manager.clone();
        agent_manager.lock().await.mark_done(&agent_id);

        let summary = result.map_err(|error| CrawlError::new(error.to_string()))?;
        let crawl_state = runtime.tool_executor_mut().crawl_state.clone();
        Ok(build_crawl_result(&summary, &crawl_state))
    }

    fn supports_async(tool_name: &str) -> bool {
        ToolRegistry::is_async_tool(tool_name)
    }
}

impl Drop for CrawlerAgent {
    /// Children should not outlive the parent: any handle still in
    /// `child_tasks` when the agent is dropped is aborted immediately. This
    /// is the same semantics as an explicit `cancel_subagent` (abort = no
    /// graceful drain) — see plan step 1.
    fn drop(&mut self) {
        for (_, (_, handle, _)) in self.child_tasks.drain() {
            handle.abort();
        }
    }
}

impl ToolExecutor for CrawlerAgent {
    #[allow(clippy::manual_async_fn)]
    fn execute(
        &mut self,
        tool_name: &str,
        input: &str,
    ) -> impl std::future::Future<Output = Result<ToolOutcome, ToolError>> + Send {
        async move {
            if self
                .allowed_tools
                .as_ref()
                .is_some_and(|allowed| !allowed.contains(tool_name))
            {
                return Err(ToolError::new(format!(
                    "tool `{tool_name}` is not enabled by the current allowlist"
                )));
            }

            let input_value: Value = if input.is_empty() {
                Value::Object(serde_json::Map::new())
            } else {
                serde_json::from_str(input)
                    .map_err(|error| ToolError::new(format!("invalid JSON input: {error}")))?
            };

            let tool_effect = if let Some(handler) = self.registry.get(tool_name) {
                match handler(&input_value) {
                    Ok(effect) => effect,
                    Err(error) if error.is_requires_async() => {
                        if !Self::supports_async(tool_name) {
                            // Forward the canonical phrasing to the runtime
                            // executor (which uses its own `ToolError` type).
                            return Err(ToolError::new(error.to_string()));
                        }

                        self.ensure_browser().await?;
                        let browser = self
                            .browser
                            .as_mut()
                            .ok_or_else(|| ToolError::new("browser context is not initialized"))?;
                        self.registry
                            .execute_async(tool_name, &input_value, browser)
                            .await
                            .map_err(|error| ToolError::new(error.to_string()))?
                    }
                    Err(error) => return Err(ToolError::new(error.to_string())),
                }
            } else if Self::supports_async(tool_name) {
                self.ensure_browser().await?;
                let browser = self
                    .browser
                    .as_mut()
                    .ok_or_else(|| ToolError::new("browser context is not initialized"))?;
                self.registry
                    .execute_async(tool_name, &input_value, browser)
                    .await
                    .map_err(|error| ToolError::new(error.to_string()))?
            } else {
                return Err(ToolError::new(format!("unknown tool: `{tool_name}`")));
            };

            let observed_effect = match &tool_effect {
                ToolEffect::Reply(_) => None,
                effect => Some(effect.clone()),
            };

            self.dispatch_tool_effect(tool_effect).await.map(|text| {
                if let Some(effect) = observed_effect {
                    ToolOutcome::with_effect(text, effect)
                } else {
                    ToolOutcome::reply(text)
                }
            })
        }
    }
}

impl CrawlerAgent {
    async fn dispatch_tool_effect(&mut self, tool_effect: ToolEffect) -> Result<String, ToolError> {
        match tool_effect {
            ToolEffect::Reply(output) => Ok(output),
            ToolEffect::Spawn(spec) => self.handle_spawn(spec).await,
            ToolEffect::Wait(spec) => self.handle_wait_effect(spec).await,
            ToolEffect::Cancel(spec) => self.handle_cancel_effect(spec).await,
            ToolEffect::Status(spec) => self.handle_status_effect(spec).await,
            ToolEffect::Pause { reason } => self.handle_pause(reason).await,
        }
    }

    async fn handle_pause(&mut self, reason: String) -> Result<String, ToolError> {
        self.pause_browser_switch().await?;

        let control = self
            .control_state
            .as_ref()
            .ok_or_else(|| ToolError::new("no control state available for pause"))?
            .clone();
        control.request_pause_with_reason(&reason);

        if let Some(ref tx) = self.child_event_tx {
            let _ = tx.send(crate::child_events::ChildEvent {
                child_id: self.agent_id.clone(),
                sub_goal: String::new(),
                event: crate::child_events::ChildEventKind::PauseRequested {
                    reason: reason.clone(),
                },
            });
        }

        loop {
            tokio::select! {
                () = control.wait_for_resume() => { break; }
                () = tokio::time::sleep(tokio::time::Duration::from_millis(100)) => {
                    if !control.is_paused() { break; }
                }
            }
        }

        if let Some(ref tx) = self.child_event_tx {
            let _ = tx.send(crate::child_events::ChildEvent {
                child_id: self.agent_id.clone(),
                sub_goal: String::new(),
                event: crate::child_events::ChildEventKind::Resumed,
            });
        }

        let was_cancelled = control.is_cancelled();
        control.reset();

        if was_cancelled {
            return Err(ToolError::new("interrupted by user"));
        }

        Ok(self.auto_read_page_after_resume().await)
    }

    async fn auto_read_page_after_resume(&mut self) -> String {
        let Some(browser) = self.browser.as_mut() else {
            return serde_json::json!({
                "resumed": true,
                "page_url": "",
                "page_title": "",
                "page_content": "Browser not available after resume"
            })
            .to_string();
        };

        let bridge_result = browser.acquire_bridge().await;
        match bridge_result {
            Ok(mut bridge) => {
                let url = bridge
                    .evaluate("window.location.href")
                    .await
                    .ok()
                    .and_then(|v| v.as_str().map(String::from))
                    .unwrap_or_default();
                let title = bridge
                    .evaluate("document.title")
                    .await
                    .ok()
                    .and_then(|v| v.as_str().map(String::from))
                    .unwrap_or_default();
                match bridge.read_content(None, None, 0, 4000).await {
                    Ok(content) => {
                        let text = content
                            .get("text")
                            .and_then(|t| t.as_str())
                            .unwrap_or("")
                            .to_string();
                        serde_json::json!({
                            "resumed": true,
                            "page_url": url,
                            "page_title": title,
                            "page_content": text
                        })
                        .to_string()
                    }
                    Err(e) => serde_json::json!({
                        "resumed": true,
                        "page_url": url,
                        "page_title": title,
                        "page_content": "",
                        "read_error": e.to_string()
                    })
                    .to_string(),
                }
            }
            Err(_) => serde_json::json!({
                "resumed": true,
                "page_url": "",
                "page_title": "",
                "page_content": "Browser not available after resume"
            })
            .to_string(),
        }
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

    let extracted_data = crawl_state.all_data();

    CrawlResult {
        summary: text_summary,
        extracted_data,
        steps_executed: summary.iterations,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::OnceLock;

    use acrawl_core::{ApiRequest, AssistantEvent, RuntimeError, TokenUsage};
    use tokio::sync::Mutex as AsyncMutex;

    use super::*;
    use crate::registry::ToolRegistry;

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

    fn mock_registry() -> ToolRegistry {
        let mut registry = ToolRegistry::new();
        registry.register(
            "navigate",
            Box::new(|input| {
                let url = input
                    .get("url")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                Ok(ToolEffect::Reply(format!("Navigated to {url}")))
            }),
        );
        registry.register(
            "extract_data",
            Box::new(|input| {
                let data = input.get("data").cloned().unwrap_or(Value::Null);
                Ok(ToolEffect::reply_json(
                    &serde_json::json!({"title": "Example", "extracted": data}),
                ))
            }),
        );
        registry.register(
            "wait_for_subagents",
            Box::new(crate::tools::wait_for_subagents::execute),
        );
        registry
    }

    fn env_lock() -> &'static AsyncMutex<()> {
        static LOCK: OnceLock<AsyncMutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| AsyncMutex::new(()))
    }

    #[tokio::test]
    async fn test_reply_effect_adds_to_conversation() {
        let mut agent = CrawlerAgent::new_for_testing(mock_registry());
        let result = agent
            .dispatch_tool_effect(ToolEffect::Reply("reply payload".to_string()))
            .await
            .expect("reply effect should return output");

        assert_eq!(result, "reply payload");
    }

    #[tokio::test]
    async fn test_spawn_effect_triggers_fork() {
        let _env_guard = env_lock().lock().await;
        std::env::set_var("HEADLESS", "true");
        let manager = default_agent_manager();
        manager.lock().await.register_root("root");

        let mut agent = CrawlerAgent::new_for_testing(ToolRegistry::new_with_core_tools())
            .with_agent_id("root".to_string())
            .with_agent_manager(manager.clone());
        agent.api_client_arc = Some(SharedApiClient::new(TextOnlyApiClient));
        agent.shared_bridge = Some(Arc::new(Mutex::new(Box::new(
            crate::PlaywrightBridge::new()
                .await
                .expect("bridge should initialize for spawn test"),
        )
            as Box<dyn BrowserBackend + Send>)));
        agent.fork_page_index_override = Some(1);

        let observation = agent
            .dispatch_tool_effect(ToolEffect::Spawn(crate::CrawlTask {
                objective: "collect details".to_string(),
                scope: crate::CrawlScope::SinglePage {
                    url: "https://example.com".to_string(),
                },
                max_steps: None,
                page_index: None,
            }))
            .await
            .expect("spawn effect should fork child");

        assert_eq!(
            observation,
            "Forked subagent root-child-1 for: collect details"
        );
        assert_eq!(agent.child_tasks.len(), 1);
        for (_, (_, handle, _)) in agent.child_tasks.drain() {
            handle.abort();
        }
    }

    #[tokio::test]
    async fn test_wait_effect_triggers_join() {
        let manager = default_agent_manager();
        manager.lock().await.register_root("test-agent");

        let mut agent = CrawlerAgent::new_for_testing(mock_registry()).with_agent_manager(manager);
        let handle = tokio::spawn(async { Some(vec![serde_json::json!({"child": 1})]) });
        agent
            .child_tasks
            .insert("child-1".to_string(), ("goal".to_string(), handle, None));

        let result = agent
            .dispatch_tool_effect(ToolEffect::Wait(crate::WaitSpec {
                child_ids: Some(vec!["child-1".to_string()]),
            }))
            .await
            .expect("wait effect should collect child results");

        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["waited"], 1);
        assert_eq!(parsed["finished"][0]["items_extracted"], 1);
        assert_eq!(agent.crawl_state.child_blocks.len(), 1);
    }

    #[tokio::test]
    async fn pause_with_children_errors() {
        let manager = default_agent_manager();
        {
            let mut locked = manager.lock().await;
            locked.register_root("test-agent");
            locked
                .register_child("test-agent-child-1", "test-agent", None)
                .expect("child registration should succeed");
        }

        let _env_guard = env_lock().lock().await;
        std::env::set_var("HEADLESS", "true");

        let control_state = Arc::new(ControlState::default());
        let mut agent = CrawlerAgent::new_for_testing(mock_registry())
            .with_agent_manager(manager)
            .with_control_state(control_state.clone());
        let err = agent
            .dispatch_tool_effect(ToolEffect::Pause {
                reason: "need human".to_string(),
            })
            .await
            .expect_err("pause should fail while child agents are active");

        assert!(
            err.to_string()
                .contains("Cannot pause while sub-agents are running"),
            "unexpected error: {err}"
        );
        assert!(!control_state.is_paused());
    }

    #[tokio::test]
    async fn pause_when_already_headed_requests_runtime_pause() {
        let _env_guard = env_lock().lock().await;
        std::env::set_var("HEADLESS", "false");

        let control_state = Arc::new(ControlState::default());
        let control_clone = Arc::clone(&control_state);
        let mut agent = CrawlerAgent::new_for_testing(mock_registry())
            .with_control_state(control_state.clone());

        tokio::spawn(async move {
            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
            control_clone.resume();
        });

        let result = agent
            .dispatch_tool_effect(ToolEffect::Pause {
                reason: "manual review".to_string(),
            })
            .await
            .expect("pause should succeed when already headed");

        assert!(result.contains("\"resumed\":true") || result.contains("\"resumed\": true"));
        std::env::set_var("HEADLESS", "true");
    }

    #[tokio::test]
    async fn crawler_agent_dispatches_tool_through_registry() {
        let mut agent = CrawlerAgent::new_for_testing(mock_registry());
        let result = agent
            .execute("navigate", r#"{"url":"https://example.com"}"#)
            .await
            .expect("navigate should succeed");
        assert!(result.text.contains("Navigated to https://example.com"));
        assert!(result.effect.is_none());
    }

    #[tokio::test]
    async fn crawler_agent_rejects_disallowed_tool() {
        let mut allowed_tools = BTreeSet::new();
        allowed_tools.insert("extract_data".to_string());
        let mut agent =
            CrawlerAgent::new_for_testing(mock_registry()).with_allowed_tools(allowed_tools);
        let err = agent
            .execute("navigate", r#"{"url":"https://example.com"}"#)
            .await
            .expect_err("disallowed tool should be rejected");
        assert!(err
            .to_string()
            .contains("not enabled by the current allowlist"));
    }

    #[test]
    fn run_builds_filtered_system_prompt_when_allowed_tools_set() {
        let mut allowed = BTreeSet::new();
        allowed.insert("navigate".to_string());
        allowed.insert("screenshot".to_string());
        let agent = CrawlerAgent::new_for_testing(mock_registry()).with_allowed_tools(allowed);

        // Verify the agent has the allowlist set (system prompt filtering
        // is derived from allowed_tools inside run(); we verify the field directly)
        let tools = agent
            .allowed_tools
            .as_ref()
            .expect("allowed_tools should be set");
        assert!(tools.contains("navigate"));
        assert!(tools.contains("screenshot"));
        assert!(!tools.contains("click"));
    }

    #[tokio::test]
    async fn test_unknown_tool_returns_error() {
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
        assert!(output.text.contains("unknown"));
        assert!(output.effect.is_none());
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

        let err = CrawlerAgent::new_for_testing(mock_registry())
            .with_max_steps(3)
            .run("loop forever", InfiniteLoopApiClient)
            .await
            .expect_err("should fail due to max iterations");
        assert!(err.to_string().contains("maximum"));
    }

    #[tokio::test]
    async fn test_fork_dispatch_spawns_child() {
        let _env_guard = env_lock().lock().await;
        std::env::set_var("HEADLESS", "true");
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
        agent.shared_bridge = Some(Arc::new(Mutex::new(Box::new(
            crate::PlaywrightBridge::new()
                .await
                .expect("bridge should initialize for fork test"),
        )
            as Box<dyn BrowserBackend + Send>)));
        agent.fork_page_index_override = Some(1);

        let observation = agent
            .execute(
                "fork",
                r#"{"objective":"check result","scope":{"type":"single_page","url":"https://example.com"}}"#,
            )
            .await
            .expect("fork should succeed");

        assert!(observation
            .text
            .contains("Forked subagent root-child-1 for: check result"));
        assert!(matches!(observation.effect, Some(ToolEffect::Spawn(_))));
        assert_eq!(manager.lock().await.get_children("root").len(), 1);
        assert_eq!(agent.child_tasks.len(), 1);
        for (_, (_, handle, _)) in agent.child_tasks.drain() {
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
        let _env_guard = env_lock().lock().await;
        std::env::set_var("HEADLESS", "true");
        let manager = default_agent_manager();
        manager.lock().await.register_root("root");

        let mut agent = CrawlerAgent::new_for_testing(ToolRegistry::new_with_core_tools())
            .with_agent_id("root".to_string())
            .with_agent_manager(manager);
        agent.api_client_arc = Some(SharedApiClient::new(TextOnlyApiClient));
        agent.shared_bridge = Some(Arc::new(Mutex::new(Box::new(
            crate::PlaywrightBridge::new()
                .await
                .expect("bridge should initialize for fork test"),
        )
            as Box<dyn BrowserBackend + Send>)));
        agent.fork_page_index_override = Some(1);

        let observation = agent
            .execute(
                "fork",
                r#"{"objective":"collect details","scope":{"type":"single_page","url":"https://example.com"}}"#,
            )
            .await
            .expect("fork should succeed");

        assert_eq!(
            observation.text,
            "Forked subagent root-child-1 for: collect details"
        );
        assert!(matches!(observation.effect, Some(ToolEffect::Spawn(_))));

        for (_, (_, handle, _)) in agent.child_tasks.drain() {
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
            .execute(
                "fork",
                r#"{"objective":"blocked","scope":{"type":"single_page","url":"https://example.com"}}"#,
            )
            .await
            .expect_err("fork at limit should return an error");

        assert!(
            err.to_string().contains("active children"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn test_fork_empty_objective_returns_error() {
        let manager = default_agent_manager();
        manager.lock().await.register_root("root");

        let mut agent = CrawlerAgent::new_for_testing(ToolRegistry::new_with_core_tools())
            .with_agent_id("root".to_string())
            .with_agent_manager(manager);

        let err = agent
            .execute(
                "fork",
                r#"{"objective":"   ","scope":{"type":"single_page","url":"https://example.com"}}"#,
            )
            .await
            .expect_err("empty objective should fail");

        assert_eq!(err.to_string(), "fork requires non-empty objective");
    }

    #[tokio::test]
    async fn test_wait_no_children_returns_empty_snapshot() {
        let mut agent = CrawlerAgent::new_for_testing(mock_registry());

        let result = agent
            .handle_wait_effect(crate::WaitSpec { child_ids: None })
            .await
            .expect("wait with no children should succeed");

        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["waited"], 0);
        assert!(parsed["finished"].as_array().unwrap().is_empty());
        assert!(parsed["still_running"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_wait_records_step() {
        let manager = default_agent_manager();
        manager.lock().await.register_root("test-agent");

        let mut agent = CrawlerAgent::new_for_testing(mock_registry()).with_agent_manager(manager);
        let handle: tokio::task::JoinHandle<Option<Vec<Value>>> = tokio::spawn(async { None });
        agent
            .child_tasks
            .insert("child-1".to_string(), ("search".to_string(), handle, None));

        let result = agent.execute("wait_for_subagents", "{}").await.unwrap();

        let parsed: serde_json::Value = serde_json::from_str(&result.text).unwrap();
        assert_eq!(parsed["waited"], 1);
        assert_eq!(parsed["finished"].as_array().unwrap().len(), 1);
        assert_eq!(agent.crawl_state.action_history.len(), 1);
        assert!(agent.crawl_state.action_history[0].contains("Waited on 1 subagent(s)"));
        assert!(matches!(result.effect, Some(ToolEffect::Wait(_))));
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
            .insert("child-1".to_string(), ("search".to_string(), handle, None));

        let result = agent.execute("wait_for_subagents", "{}").await.unwrap();

        let parsed: serde_json::Value = serde_json::from_str(&result.text).unwrap();
        assert_eq!(parsed["finished"][0]["items_extracted"], 1);
        assert_eq!(agent.crawl_state.child_blocks.len(), 1);
        assert_eq!(agent.crawl_state.child_blocks[0].child_id, "child-1");
        assert_eq!(agent.crawl_state.child_blocks[0].sub_goal, "search");
        assert_eq!(agent.crawl_state.child_blocks[0].items.len(), 1);
        assert!(matches!(result.effect, Some(ToolEffect::Wait(_))));
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
            .insert("child-1".to_string(), ("goal".to_string(), handle, None));

        let _ = agent.execute("wait_for_subagents", "{}").await;

        let all = agent.crawl_state.all_data();
        assert_eq!(all.len(), 2);
    }

    #[tokio::test]
    async fn test_message_stop_terminates_naturally() {
        let result = CrawlerAgent::new_for_testing(mock_registry())
            .run("test", TextOnlyApiClient)
            .await
            .expect("text-only run should succeed");
        assert_eq!(result.steps_executed, 1);
        assert_eq!(result.summary, "All done.");
    }

    #[tokio::test]
    async fn test_fork_full_lifecycle() {
        let mut parent = CrawlerAgent::new_for_testing(mock_registry());
        parent.crawl_state.extracted_data = vec![serde_json::json!({"parent": 1})];

        let handle: tokio::task::JoinHandle<Option<Vec<Value>>> = tokio::spawn(async {
            Some(vec![
                serde_json::json!({"child": 1}),
                serde_json::json!({"child": 2}),
            ])
        });
        parent.child_tasks.insert(
            "child-1".to_string(),
            ("search page 2".to_string(), handle, None),
        );

        let wait_result = parent.execute("wait_for_subagents", "{}").await.unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&wait_result.text).unwrap();
        assert_eq!(parsed["finished"][0]["items_extracted"], 2);
        assert!(matches!(wait_result.effect, Some(ToolEffect::Wait(_))));

        let all = parent.crawl_state.all_data();
        assert_eq!(all.len(), 3);
        assert_eq!(parent.crawl_state.child_blocks.len(), 1);
        assert_eq!(parent.crawl_state.child_blocks[0].sub_goal, "search page 2");
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
        assert!(!mgr.can_fork("grandchild1"));
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
            assistant_messages: vec![runtime::ConversationMessage::assistant(vec![
                ContentBlock::Text {
                    text: "final answer".to_string(),
                },
            ])],
            tool_results: vec![],
            iterations: 1,
            usage: TokenUsage {
                input_tokens: 0,
                output_tokens: 0,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: 0,
            },
            auto_compaction: None,
        };
        let mut crawl_state = CrawlState {
            extracted_data: vec![serde_json::json!({"parent": 1})],
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

        crawl_state
            .extracted_data
            .push(serde_json::json!({"parent": 2}));
        let result = build_crawl_result(&summary, &crawl_state);
        assert_eq!(result.extracted_data.len(), 3);
        assert_eq!(result.extracted_data[2], serde_json::json!({"child": 1}));
    }

    #[test]
    fn build_crawl_result_ignores_non_extract_tool_results() {
        let summary = TurnSummary {
            assistant_messages: vec![runtime::ConversationMessage::assistant(vec![
                ContentBlock::Text {
                    text: "final answer".to_string(),
                },
            ])],
            tool_results: vec![runtime::ConversationMessage::tool_result(
                "call-1".to_string(),
                "navigate".to_string(),
                r#"{"data":{"ignored":true}}"#.to_string(),
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
            extracted_data: vec![serde_json::json!({"from_state": true})],
            ..CrawlState::default()
        };

        let result = build_crawl_result(&summary, &crawl_state);
        assert_eq!(
            result.extracted_data,
            vec![serde_json::json!({"from_state": true})]
        );
    }

    #[tokio::test]
    async fn wait_for_human_tool_triggers_agent_pause() {
        let _env_guard = env_lock().lock().await;
        std::env::set_var("HEADLESS", "false");

        let control_state = Arc::new(ControlState::default());
        let control_clone = Arc::clone(&control_state);

        let mut registry = ToolRegistry::new();
        registry.register(
            "wait_for_human",
            Box::new(|input| crate::tools::wait_for_human::execute(input, true)),
        );

        let mut agent =
            CrawlerAgent::new_for_testing(registry).with_control_state(control_state.clone());

        assert!(!control_state.is_paused());

        tokio::spawn(async move {
            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
            control_clone.resume();
        });

        let result = agent
            .execute("wait_for_human", r#"{"reason":"captcha detected"}"#)
            .await
            .expect("wait_for_human should succeed in interactive mode");

        assert!(
            result.text.contains("resumed"),
            "result should contain resumed status: {result:?}"
        );
        assert!(matches!(result.effect, Some(ToolEffect::Pause { .. })));
        std::env::set_var("HEADLESS", "true");
    }

    #[tokio::test]
    async fn pause_no_control_state_returns_error() {
        let _env_guard = env_lock().lock().await;
        std::env::set_var("HEADLESS", "false");

        let mut agent = CrawlerAgent::new_for_testing(mock_registry());

        let err = agent
            .dispatch_tool_effect(ToolEffect::Pause {
                reason: "test".to_string(),
            })
            .await
            .expect_err("pause should fail without control state");

        assert!(
            err.to_string().contains("no control state"),
            "unexpected error: {err}"
        );
        std::env::set_var("HEADLESS", "true");
    }
}
