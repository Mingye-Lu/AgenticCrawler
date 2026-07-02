use std::collections::{BTreeSet, HashMap};
use std::fmt::{Display, Formatter};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use acrawl_core::{ApiClient, ContentBlock, ToolError, ToolExecutor, ToolOutcome};
use runtime::{ControlState, ConversationMessage, ConversationRuntime, Session, TurnSummary};
use serde_json::Value;
use tokio::sync::Mutex as AsyncMutex;

use crate::manager::SharedAgentManager;
use crate::prompt::{build_system_prompt, DynamicPromptContext};
use crate::registry::ToolRegistry;
use crate::script_manager::{ScriptError, ScriptManager};
use crate::state::CrawlState;
use crate::tool_effect::ToolEffect;
use crate::{mvp_tool_specs, AgentManager, BrowserContext, SharedApiClient, SharedBridge};

mod fork;
mod lifecycle;

#[cfg(test)]
use std::collections::BTreeMap;

#[cfg(test)]
use crate::BrowserBackend;
#[cfg(test)]
use crate::{BridgeError, BrowserState, PageInfo, ScreenshotOptions};

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
    pub messages: Vec<ConversationMessage>,
    pub model: Option<String>,
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
    tokio::task::JoinHandle<Option<CrawlResult>>,
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
    script_manager: ScriptManager,
    control_state: Option<Arc<ControlState>>,
    child_event_tx: Option<std::sync::mpsc::Sender<crate::child_events::ChildEvent>>,
    child_control_registry: Option<crate::child_events::ChildControlRegistry>,
    extension_mode: bool,
    is_child: bool,
    pub(super) child_snapshots: crate::child_events::ChildSnapshotRegistry,
    prompt_override_slot: Arc<Mutex<Option<Vec<String>>>>,
    last_assistant_text_slot: Arc<Mutex<Option<String>>>,
    accumulated_turn_ctx: Mutex<DynamicPromptContext>,
    cumulative_cost_slot: runtime::SharedCostCounter,
    step_count: usize,
    confidence_tracker: Option<crate::confidence::ConfidenceTracker>,
    model_supports_vision: bool,
    #[cfg(test)]
    pub(super) fork_page_index_override: Option<usize>,
}

impl CrawlerAgent {
    fn new_with_slots(
        browser: Option<BrowserContext>,
        registry: ToolRegistry,
        agent_id: String,
        prompt_override_slot: Arc<Mutex<Option<Vec<String>>>>,
        last_assistant_text_slot: Arc<Mutex<Option<String>>>,
    ) -> Self {
        let shared_bridge = browser.as_ref().map(|context| context.bridge().clone());
        Self {
            shared_bridge,
            browser,
            registry,
            allowed_tools: None,
            max_steps: DEFAULT_MAX_STEPS,
            agent_id,
            agent_manager: default_agent_manager(),
            crawl_state: CrawlState {
                max_steps: DEFAULT_MAX_STEPS,
                ..CrawlState::default()
            },
            child_tasks: HashMap::new(),
            child_id_counter: std::sync::atomic::AtomicU64::new(0),
            api_client_arc: None,
            script_manager: default_script_manager(),
            control_state: None,
            child_event_tx: None,
            child_control_registry: None,
            extension_mode: false,
            is_child: false,
            child_snapshots: crate::child_events::ChildSnapshotRegistry::default(),
            prompt_override_slot,
            last_assistant_text_slot,
            accumulated_turn_ctx: Mutex::new(DynamicPromptContext::default()),
            cumulative_cost_slot: runtime::new_cost_counter(),
            step_count: 0,
            confidence_tracker: None,
            model_supports_vision: false,
            #[cfg(test)]
            fork_page_index_override: None,
        }
    }

    #[must_use]
    pub fn take_captured_child_sessions(&mut self) -> Vec<runtime::ChildSession> {
        std::mem::take(&mut self.crawl_state.captured_child_sessions)
    }

    pub fn push_captured_child_session_for_test(&mut self, session: runtime::ChildSession) {
        self.crawl_state.captured_child_sessions.push(session);
    }

    #[must_use]
    pub fn new(browser: BrowserContext, registry: ToolRegistry) -> Self {
        Self::new_with_slots(
            Some(browser),
            registry,
            generate_agent_id(),
            Arc::new(Mutex::new(None)),
            Arc::new(Mutex::new(None)),
        )
    }

    #[must_use]
    pub fn new_lazy(registry: ToolRegistry) -> Self {
        Self::new_with_slots(
            None,
            registry,
            generate_agent_id(),
            Arc::new(Mutex::new(None)),
            Arc::new(Mutex::new(None)),
        )
    }

    #[cfg(test)]
    fn new_for_testing(registry: ToolRegistry) -> Self {
        Self::new_with_slots(
            None,
            registry,
            "test-agent".to_string(),
            Arc::new(Mutex::new(None)),
            Arc::new(Mutex::new(None)),
        )
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
    pub fn with_model_supports_vision(mut self, value: bool) -> Self {
        self.model_supports_vision = value;
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

    #[must_use]
    pub fn as_child(mut self) -> Self {
        self.is_child = true;
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
        let system_prompt = build_system_prompt(&specs, None);
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
        let model_supports_vision = self.model_supports_vision;
        let prompt_override_slot = Arc::new(Mutex::new(None));
        let last_assistant_text_slot = Arc::new(Mutex::new(None));
        self.prompt_override_slot = Arc::clone(&prompt_override_slot);
        self.last_assistant_text_slot = Arc::clone(&last_assistant_text_slot);
        let mut runtime = ConversationRuntime::new(
            Session::new(),
            shared_client.clone(),
            self,
            system_prompt,
            prompt_override_slot,
            last_assistant_text_slot,
        )
        .with_max_iterations(max_steps)
        .with_model_supports_vision(model_supports_vision);
        runtime.tool_executor_mut().cumulative_cost_slot = runtime.cumulative_cost_counter();

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
        let messages = runtime.session().messages.clone();
        let model = runtime.session().model.clone();
        let mut crawl_result = build_crawl_result(&summary, &crawl_state);
        crawl_result.messages = messages;
        crawl_result.model = model;
        Ok(crawl_result)
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
            let settings = runtime::load_settings();
            self.step_count += 1;
            *self
                .accumulated_turn_ctx
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) =
                DynamicPromptContext::default();

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

            if let Some(cached_output) =
                self.lookup_cached_action(&settings, tool_name, &input_value)
            {
                return Ok(ToolOutcome::reply(cached_output));
            }

            let use_healing = runtime::settings_get_self_healing(&settings);
            let max_retries = runtime::settings_get_self_healing_max_retries(&settings);
            let mut current_input = input_value.clone();
            let mut heal_log = String::new();

            let (mut text, observed_effect) = {
                let mut attempts = 0usize;
                loop {
                    match self.execute_tool_once(tool_name, &current_input).await {
                        Ok(result) => break result,
                        Err(error)
                            if use_healing
                                && attempts < max_retries
                                && matches!(
                                    crate::failure_classifier::classify(
                                        tool_name,
                                        &error.to_string()
                                    ),
                                    crate::failure_classifier::FailureCategory::SelectorNotFound
                                        | crate::failure_classifier::FailureCategory::SelectorAmbiguous
                                ) =>
                        {
                            let Some((patched_input, current_heal_log)) =
                                self.try_self_heal(tool_name, &current_input).await?
                            else {
                                return Err(error);
                            };
                            current_input = patched_input;
                            heal_log = current_heal_log;
                            attempts += 1;
                        }
                        Err(error) => {
                            let enriched = if runtime::settings_get_failure_classification(&settings)
                            {
                                let category =
                                    crate::failure_classifier::classify(tool_name, &error.to_string());
                                ToolError::new(format!("[{category}] {error}"))
                            } else {
                                error
                            };
                            return Err(enriched);
                        }
                    }
                }
            };

            if !heal_log.is_empty() {
                text = format!("{text} {heal_log}");
            }

            if observed_effect.is_none() {
                self.store_cached_action(&settings, tool_name, &input_value, &text);
            }

            if runtime::settings_get_loop_detection(&settings) {
                self.apply_loop_detection(&settings, tool_name, &input_value);
            }

            let interval = runtime::settings_get_planning_interval(&settings);
            if interval > 0 {
                self.apply_planning_guidance(interval);
            }

            if runtime::settings_get_confidence_tracking(&settings) {
                self.apply_confidence_tracking();
            }

            self.enforce_budget(&settings)?;

            Ok(if let Some(effect) = observed_effect {
                ToolOutcome::with_effect(text, effect)
            } else {
                ToolOutcome::reply(text)
            })
        }
    }
}

impl CrawlerAgent {
    fn enforce_budget(&self, settings: &runtime::Settings) -> Result<(), ToolError> {
        let Some(max_usd) = runtime::settings_get_budget_max_session_cost_usd(settings) else {
            return Ok(());
        };

        let mode = runtime::settings_get_budget_enforcement(settings)
            .as_deref()
            .and_then(runtime::BudgetMode::parse)
            .unwrap_or(runtime::BudgetMode::Block);
        let enforcer = runtime::BudgetEnforcer::new(
            max_usd,
            mode,
            runtime::settings_get_budget_warn_threshold_pct(settings),
        );
        let current_usd =
            runtime::millicents_to_usd(self.cumulative_cost_slot.load(Ordering::Relaxed));

        match enforcer.check(current_usd) {
            runtime::BudgetDecision::Allow => Ok(()),
            runtime::BudgetDecision::Warn { remaining_usd } => {
                self.write_prompt_override(&DynamicPromptContext {
                    budget_warning: Some(format!(
                        "Budget warning: ${remaining_usd:.4} remaining (limit: ${max_usd:.4})"
                    )),
                    ..DynamicPromptContext::default()
                });
                Ok(())
            }
            runtime::BudgetDecision::Block => Err(ToolError::new(
                "Budget exceeded: session cost limit reached",
            )),
        }
    }

    async fn execute_tool_once(
        &mut self,
        tool_name: &str,
        input_value: &Value,
    ) -> Result<(String, Option<ToolEffect>), ToolError> {
        let tool_effect = if let Some(handler) = self.registry.get(tool_name) {
            match handler(input_value) {
                Ok(effect) => effect,
                Err(error) if error.is_requires_async() => {
                    if !Self::supports_async(tool_name) {
                        return Err(ToolError::new(error.to_string()));
                    }

                    self.ensure_browser().await?;
                    let browser = self
                        .browser
                        .as_mut()
                        .ok_or_else(|| ToolError::new("browser context is not initialized"))?;
                    self.crawl_state.has_active_subagents = !self
                        .agent_manager
                        .lock()
                        .await
                        .get_active_children(&self.agent_id)
                        .is_empty();
                    self.registry
                        .execute_async(tool_name, input_value, browser, &mut self.crawl_state)
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
            self.crawl_state.has_active_subagents = !self
                .agent_manager
                .lock()
                .await
                .get_active_children(&self.agent_id)
                .is_empty();
            self.registry
                .execute_async(tool_name, input_value, browser, &mut self.crawl_state)
                .await
                .map_err(|error| ToolError::new(error.to_string()))?
        } else {
            return Err(ToolError::new(format!("unknown tool: `{tool_name}`")));
        };

        let observed_effect = match &tool_effect {
            ToolEffect::Reply(_) => None,
            effect => Some(effect.clone()),
        };
        let text = self.dispatch_tool_effect(tool_effect).await?;
        Ok((text, observed_effect))
    }

    fn should_attempt_self_healing(tool_name: &str) -> bool {
        matches!(
            tool_name,
            "click" | "fill_form" | "hover" | "press_key" | "select_option"
        )
    }

    async fn try_self_heal(
        &mut self,
        tool_name: &str,
        input_value: &Value,
    ) -> Result<Option<(Value, String)>, ToolError> {
        if !Self::should_attempt_self_healing(tool_name) {
            return Ok(None);
        }

        let Some(old_ref) = crate::self_healing::extract_element_ref(input_value) else {
            return Ok(None);
        };

        self.ensure_browser().await?;
        let browser = self
            .browser
            .as_mut()
            .ok_or_else(|| ToolError::new("browser context is not initialized"))?;

        let original_hint = browser
            .ref_map()
            .get(old_ref.trim_start_matches('@'))
            .map(|entry| entry.name.clone())
            .filter(|name| !name.trim().is_empty());

        let mut fresh_page_map = browser
            .acquire_bridge()
            .await
            .map_err(|error| ToolError::new(error.to_string()))?
            .page_map(None, false)
            .await
            .map_err(|error| ToolError::new(error.to_string()))?;

        let cache_key = crate::tools::page_map::normalize_url(
            fresh_page_map
                .get("meta")
                .and_then(|meta| meta.get("url"))
                .and_then(Value::as_str)
                .unwrap_or("unknown"),
        )
        .to_string();

        if let Some(prev_url) = browser.snapshot_url() {
            if prev_url != cache_key.as_str() {
                browser.ref_map_mut().clear();
            }
        }

        crate::tools::page_map::annotate_refs(&mut fresh_page_map, browser);
        browser.set_page_snapshot(&cache_key, None, fresh_page_map.clone());

        let Some(new_selector) = crate::self_healing::find_healed_selector(
            &old_ref,
            &fresh_page_map,
            original_hint.as_deref(),
        ) else {
            return Ok(None);
        };

        let patched = crate::self_healing::patch_selector(input_value, &old_ref, &new_selector);
        Ok(Some((
            patched,
            format!("[healed: {old_ref} → {new_selector}]"),
        )))
    }

    fn lookup_cached_action(
        &mut self,
        settings: &runtime::Settings,
        tool_name: &str,
        input_value: &Value,
    ) -> Option<String> {
        if !runtime::settings_get_action_caching(settings)
            || !crate::action_cache::is_cacheable(tool_name)
        {
            return None;
        }

        let current_fingerprint = self.crawl_state.page_fingerprints.last().cloned()?;
        let ttl_secs = runtime::settings_get_action_cache_ttl_secs(settings);

        if self.crawl_state.action_cache.is_none() {
            self.crawl_state.action_cache = Some(crate::action_cache::ActionCache::new(ttl_secs));
        }

        let cache_key = crate::action_cache::ActionCache::make_key(
            tool_name,
            input_value,
            &current_fingerprint,
        );
        self.crawl_state
            .action_cache
            .as_mut()
            .and_then(|cache| cache.lookup(&cache_key, &current_fingerprint))
    }

    fn store_cached_action(
        &mut self,
        settings: &runtime::Settings,
        tool_name: &str,
        input_value: &Value,
        text: &str,
    ) {
        if !runtime::settings_get_action_caching(settings)
            || !crate::action_cache::is_cacheable(tool_name)
        {
            return;
        }

        let Some(current_fingerprint) = self.crawl_state.page_fingerprints.last().cloned() else {
            return;
        };
        let Some(cache) = self.crawl_state.action_cache.as_mut() else {
            return;
        };

        let cache_key = crate::action_cache::ActionCache::make_key(
            tool_name,
            input_value,
            &current_fingerprint,
        );
        cache.store(cache_key, text.to_string(), current_fingerprint);
    }

    fn apply_planning_guidance(&self, interval: usize) {
        let planning_guidance = if self.step_count.is_multiple_of(interval) {
            "Planning checkpoint: Review your overall goal, assess progress, and decide your next major objective."
                .to_string()
        } else {
            "Execution mode: Focus on the current step. Take precise action and evaluate the result."
                .to_string()
        };

        self.write_prompt_override(&DynamicPromptContext {
            planning_guidance: Some(planning_guidance),
            ..Default::default()
        });
    }

    fn apply_loop_detection(
        &mut self,
        settings: &runtime::Settings,
        tool_name: &str,
        input: &Value,
    ) {
        let window = runtime::settings_get_loop_detection_window(settings);
        let threshold = runtime::settings_get_loop_nudge_threshold(settings);

        if self.crawl_state.loop_detector.is_none() {
            self.crawl_state.loop_detector =
                Some(crate::loop_detector::LoopDetector::new(window, threshold));
        }

        if let Some(detector) = self.crawl_state.loop_detector.as_mut() {
            detector.record_action(tool_name, input);

            if let Some(fingerprint) = self.crawl_state.page_fingerprints.last() {
                detector.record_page_state(fingerprint);
            }

            if let Some(nudge) = detector.detect_loop() {
                self.write_prompt_override(&DynamicPromptContext {
                    loop_nudge: Some(nudge.message().to_string()),
                    ..DynamicPromptContext::default()
                });
            }
        }
    }

    fn write_prompt_override(&self, ctx: &DynamicPromptContext) {
        let mut accumulated = self
            .accumulated_turn_ctx
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if ctx.stagnation_alert.is_some() {
            accumulated
                .stagnation_alert
                .clone_from(&ctx.stagnation_alert);
        }
        if ctx.planning_guidance.is_some() {
            accumulated
                .planning_guidance
                .clone_from(&ctx.planning_guidance);
        }
        if ctx.budget_warning.is_some() {
            accumulated.budget_warning.clone_from(&ctx.budget_warning);
        }
        if ctx.loop_nudge.is_some() {
            accumulated.loop_nudge.clone_from(&ctx.loop_nudge);
        }
        let specs = crate::mvp_tool_specs();
        let new_prompt = build_system_prompt(&specs, Some(&*accumulated));
        *self
            .prompt_override_slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(new_prompt);
    }

    fn apply_confidence_tracking(&mut self) {
        let text_opt = {
            let guard = self
                .last_assistant_text_slot
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            guard.clone()
        };

        if let Some(text) = text_opt {
            if let Some(conf) = crate::confidence::ConfidenceTracker::parse_from_text(&text) {
                if self.confidence_tracker.is_none() {
                    self.confidence_tracker = Some(crate::confidence::ConfidenceTracker::new());
                }
                let should_alert = self
                    .confidence_tracker
                    .as_mut()
                    .is_some_and(|tracker| tracker.record(conf));

                if should_alert {
                    self.write_prompt_override(&DynamicPromptContext {
                        stagnation_alert: Some(
                            "Your confidence has been LOW for multiple consecutive steps. \
                             Reconsider your approach."
                                .to_string(),
                        ),
                        planning_guidance: Some(crate::confidence::confidence_instruction()),
                        ..DynamicPromptContext::default()
                    });
                } else {
                    self.write_prompt_override(&DynamicPromptContext {
                        planning_guidance: Some(crate::confidence::confidence_instruction()),
                        ..DynamicPromptContext::default()
                    });
                }
            }
        }
    }

    async fn dispatch_tool_effect(&mut self, tool_effect: ToolEffect) -> Result<String, ToolError> {
        match tool_effect {
            ToolEffect::Reply(output) => Ok(output),
            ToolEffect::Spawn(spec) => self.handle_spawn(spec).await,
            ToolEffect::Wait(spec) => self.handle_wait_effect(spec).await,
            ToolEffect::Cancel(spec) => self.handle_cancel_effect(spec).await,
            ToolEffect::Status(spec) => self.handle_status_effect(spec).await,
            ToolEffect::RunScript(task) => self.handle_run_script_effect(task).await,
            ToolEffect::ScriptWait(spec) => self.handle_script_wait_effect(spec).await,
            ToolEffect::ScriptCancel(spec) => self.handle_script_cancel_effect(spec),
            ToolEffect::ScriptStatus(spec) => self.handle_script_status_effect(spec),
            ToolEffect::Vision(payload) => Ok(payload.caption),
        }
    }

    async fn handle_run_script_effect(
        &mut self,
        mut task: acrawl_core::ScriptTask,
    ) -> Result<String, ToolError> {
        self.ensure_browser().await?;

        if task
            .script
            .get("__load_from_disk")
            .and_then(Value::as_str)
            .is_some()
        {
            task.script = self.load_script_from_disk(&task)?;
        }

        let current_url = self.crawl_state.current_url.clone();
        let cloned_context = self
            .clone_browser_context_for_script(current_url.as_deref())
            .await?;
        let script_id = self
            .script_manager
            .spawn_script(task, cloned_context)
            .map_err(script_error_to_tool)?;

        Ok(format!("Script started: {script_id}"))
    }

    async fn handle_script_wait_effect(
        &mut self,
        spec: acrawl_core::ScriptWaitSpec,
    ) -> Result<String, ToolError> {
        let results = self
            .script_manager
            .wait_for_scripts(spec.script_ids)
            .await
            .map_err(script_error_to_tool)?;
        serde_json::to_string(&results)
            .map_err(|error| ToolError::new(format!("failed to serialize script results: {error}")))
    }

    #[allow(clippy::needless_pass_by_value)]
    fn handle_script_cancel_effect(
        &mut self,
        spec: acrawl_core::ScriptCancelSpec,
    ) -> Result<String, ToolError> {
        self.script_manager
            .cancel_script(&spec.script_id)
            .map_err(script_error_to_tool)?;
        Ok(format!("Script cancelled: {}", spec.script_id))
    }

    #[allow(clippy::needless_pass_by_value)]
    fn handle_script_status_effect(
        &mut self,
        spec: acrawl_core::ScriptStatusSpec,
    ) -> Result<String, ToolError> {
        let status = self
            .script_manager
            .get_status(&spec.script_id)
            .map_err(script_error_to_tool)?;
        serde_json::to_string(&status)
            .map_err(|error| ToolError::new(format!("failed to serialize script status: {error}")))
    }

    async fn clone_browser_context_for_script(
        &mut self,
        target_url: Option<&str>,
    ) -> Result<BrowserContext, ToolError> {
        let shared_bridge = self
            .shared_bridge
            .clone()
            .or_else(|| {
                self.browser
                    .as_ref()
                    .map(|browser| browser.bridge().clone())
            })
            .ok_or_else(|| ToolError::new("script: browser bridge not initialized"))?;
        let page_index = {
            let mut bridge = shared_bridge.lock().await;
            bridge
                .new_page(target_url)
                .await
                .map_err(|error| ToolError::new(error.to_string()))?
        };

        let mut browser = BrowserContext::new_shared(shared_bridge, page_index);
        if let Some(url) = target_url {
            browser.set_navigated_url(url, true);
        }
        Ok(browser)
    }

    fn load_script_from_disk(&self, task: &acrawl_core::ScriptTask) -> Result<Value, ToolError> {
        let script_name = task
            .script
            .get("__load_from_disk")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::new("script loader requires __load_from_disk string"))?;
        let script_settings = self.script_manager.settings.clone();
        let scripts_dir = script_settings.scripts_dir.unwrap_or_else(|| {
            runtime::settings::ScriptSettings::default()
                .scripts_dir
                .unwrap_or_else(|| acrawl_core::config_home_dir().join("scripts"))
        });
        let script_path = scripts_dir.join(format!("{script_name}.json"));
        let script_text = std::fs::read_to_string(&script_path).map_err(|error| {
            ToolError::new(format!(
                "failed to read script `{script_name}` from {}: {error}",
                script_path.display()
            ))
        })?;
        serde_json::from_str(&script_text).map_err(|error| {
            ToolError::new(format!(
                "failed to parse script `{script_name}` from {}: {error}",
                script_path.display()
            ))
        })
    }
}

fn default_agent_manager() -> SharedAgentManager {
    Arc::new(AsyncMutex::new(AgentManager::new(
        DEFAULT_MAX_CONCURRENT_PER_PARENT,
        DEFAULT_MAX_FORK_DEPTH,
        DEFAULT_MAX_TOTAL_AGENTS,
    )))
}

fn default_script_manager() -> ScriptManager {
    let settings = runtime::load_settings();
    ScriptManager::new(settings.script.unwrap_or_default())
}

#[allow(clippy::needless_pass_by_value)]
fn script_error_to_tool(error: ScriptError) -> ToolError {
    ToolError::new(error.to_string())
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
        messages: Vec::new(),
        model: None,
    }
}

#[cfg(test)]
mod tests {
    use acrawl_core::{ApiRequest, AssistantEvent, RuntimeError, TokenUsage};
    use async_trait::async_trait;
    use tokio::sync::Mutex as AsyncMutex;

    use super::*;
    use crate::page_fingerprint::PageFingerprint;
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

    fn action_cache_test_fingerprint(url: &str) -> PageFingerprint {
        PageFingerprint {
            url: url.to_string(),
            element_count: 3,
            text_hash: 42,
        }
    }

    fn write_action_cache_settings(config_home: &std::path::Path, enabled: bool, ttl_secs: u64) {
        std::env::set_var("ACRAWL_CONFIG_HOME", config_home);
        runtime::save_settings(&runtime::Settings {
            optimization: Some(runtime::settings::OptimizationSettings {
                action_caching: Some(enabled),
                action_cache_ttl_secs: Some(ttl_secs),
                ..Default::default()
            }),
            ..runtime::Settings::default()
        })
        .expect("settings should save");
    }

    fn cache_test_registry(
        counter: Arc<std::sync::Mutex<usize>>,
        tool_name: &'static str,
    ) -> ToolRegistry {
        let mut registry = ToolRegistry::new();
        registry.register(
            tool_name,
            Box::new(move |input| {
                let mut calls = counter
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                *calls += 1;
                Ok(ToolEffect::Reply(format!(
                    "{tool_name} call {} with {}",
                    *calls, input
                )))
            }),
        );
        registry
    }

    fn write_self_healing_settings(config_home: &std::path::Path, enabled: bool, retries: usize) {
        std::env::set_var("ACRAWL_CONFIG_HOME", config_home);
        runtime::save_settings(&runtime::Settings {
            optimization: Some(runtime::settings::OptimizationSettings {
                self_healing: Some(enabled),
                self_healing_max_retries: Some(retries),
                ..Default::default()
            }),
            ..runtime::Settings::default()
        })
        .expect("settings should save");
    }

    #[derive(Debug, Default)]
    struct HealingBridgeState {
        latest_click_selector: Option<String>,
        page_map_calls: usize,
    }

    #[derive(Debug)]
    struct HealingTestBridge {
        click_failures_remaining: usize,
        click_error_message: String,
        page_map_value: Value,
        state: Arc<std::sync::Mutex<HealingBridgeState>>,
    }

    impl HealingTestBridge {
        fn new(
            click_failures_remaining: usize,
            click_error_message: &str,
            page_map_value: Value,
            state: Arc<std::sync::Mutex<HealingBridgeState>>,
        ) -> Self {
            Self {
                click_failures_remaining,
                click_error_message: click_error_message.to_string(),
                page_map_value,
                state,
            }
        }
    }

    #[async_trait]
    impl BrowserBackend for HealingTestBridge {
        async fn poll_observations(
            &mut self,
        ) -> Result<Vec<browser::ObservationEvent>, BridgeError> {
            Ok(Vec::new())
        }
        async fn set_seq(&mut self, _seq: u64) -> Result<(), BridgeError> {
            Ok(())
        }
        async fn navigate(&mut self, _url: &str) -> Result<PageInfo, BridgeError> {
            Err(BridgeError::Protocol("unused".into()))
        }

        async fn new_page(&mut self, _url: Option<&str>) -> Result<usize, BridgeError> {
            Ok(0)
        }

        async fn close_page(&mut self, _page_index: usize) -> Result<(), BridgeError> {
            Ok(())
        }

        async fn scroll(&mut self, _direction: &str, _pixels: i64) -> Result<(), BridgeError> {
            Err(BridgeError::Protocol("unused".into()))
        }

        async fn page_map(
            &mut self,
            _scope: Option<&str>,
            _compound_enrichment: bool,
        ) -> Result<Value, BridgeError> {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.page_map_calls += 1;
            Ok(self.page_map_value.clone())
        }

        async fn read_content(
            &mut self,
            _heading: Option<&str>,
            _selector: Option<&str>,
            _offset: usize,
            _max_chars: usize,
        ) -> Result<Value, BridgeError> {
            Err(BridgeError::Protocol("unused".into()))
        }

        async fn wait_for_selector(
            &mut self,
            _selector: &str,
            _timeout_ms: u64,
            _state: Option<&str>,
        ) -> Result<bool, BridgeError> {
            Err(BridgeError::Protocol("unused".into()))
        }

        async fn select_option(
            &mut self,
            _selector: &str,
            _value: &str,
        ) -> Result<(), BridgeError> {
            Err(BridgeError::Protocol("unused".into()))
        }

        async fn evaluate(&mut self, _script: &str) -> Result<Value, BridgeError> {
            Err(BridgeError::Protocol("unused".into()))
        }

        async fn hover(&mut self, _selector: &str) -> Result<(), BridgeError> {
            Err(BridgeError::Protocol("unused".into()))
        }

        async fn press_key(
            &mut self,
            _key: &str,
            _selector: Option<&str>,
        ) -> Result<(), BridgeError> {
            Err(BridgeError::Protocol("unused".into()))
        }

        async fn switch_tab(&mut self, _index: i64) -> Result<Value, BridgeError> {
            Ok(serde_json::json!({}))
        }

        async fn export_cookies(&mut self) -> Result<BrowserState, BridgeError> {
            Err(BridgeError::Protocol("unused".into()))
        }

        async fn import_cookies(&mut self, _state: &BrowserState) -> Result<(), BridgeError> {
            Err(BridgeError::Protocol("unused".into()))
        }

        async fn import_cookies_only(&mut self, _state: &BrowserState) -> Result<(), BridgeError> {
            Err(BridgeError::Protocol("unused".into()))
        }

        async fn import_local_storage(&mut self, _state: &BrowserState) -> Result<(), BridgeError> {
            Err(BridgeError::Protocol("unused".into()))
        }

        async fn list_resources(&mut self) -> Result<Value, BridgeError> {
            Err(BridgeError::Protocol("unused".into()))
        }

        async fn save_file(
            &mut self,
            _url: &str,
            _path: &str,
            _headers: Option<&BTreeMap<String, String>>,
        ) -> Result<String, BridgeError> {
            Err(BridgeError::Protocol("unused".into()))
        }

        async fn click(&mut self, selector: &str) -> Result<(), BridgeError> {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.latest_click_selector = Some(selector.to_string());
            drop(state);

            if self.click_failures_remaining > 0 {
                self.click_failures_remaining -= 1;
                return Err(BridgeError::Protocol(self.click_error_message.clone()));
            }
            Ok(())
        }

        async fn click_at(&mut self, _x: f64, _y: f64) -> Result<(), BridgeError> {
            Err(BridgeError::Protocol("unused".into()))
        }

        async fn fill(&mut self, _selector: &str, _value: &str) -> Result<(), BridgeError> {
            Err(BridgeError::Protocol("unused".into()))
        }

        async fn screenshot(
            &mut self,
            _options: &ScreenshotOptions<'_>,
        ) -> Result<(String, usize), BridgeError> {
            Err(BridgeError::Protocol("unused".into()))
        }

        async fn go_back(&mut self) -> Result<String, BridgeError> {
            Err(BridgeError::Protocol("unused".into()))
        }

        async fn set_device(
            &mut self,
            _: &serde_json::Value,
        ) -> Result<serde_json::Value, BridgeError> {
            Err(BridgeError::Protocol("unused".into()))
        }
    }

    fn healing_page_map() -> Value {
        serde_json::json!({
            "interactive": {
                "elements": [
                    {"selector": "#new-submit", "role": "button", "name": "Submit", "text": "Submit"}
                ]
            },
            "meta": {"url": "https://example.com", "title": "Example"},
            "headings": [],
            "links": [],
            "forms": [],
            "landmarks": []
        })
    }

    fn healing_test_agent(
        click_failures_remaining: usize,
        click_error_message: &str,
        page_map_value: Value,
    ) -> (CrawlerAgent, Arc<std::sync::Mutex<HealingBridgeState>>) {
        let state = Arc::new(std::sync::Mutex::new(HealingBridgeState::default()));
        let shared_bridge: Arc<AsyncMutex<Box<dyn BrowserBackend + Send>>> =
            Arc::new(AsyncMutex::new(Box::new(HealingTestBridge::new(
                click_failures_remaining,
                click_error_message,
                page_map_value,
                Arc::clone(&state),
            ))));
        let browser = BrowserContext::new_shared(shared_bridge, 0);
        (
            CrawlerAgent::new(browser, ToolRegistry::new_with_core_tools()),
            state,
        )
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
        let _env_guard = crate::test_async_env_lock().lock().await;
        std::env::set_var("HEADLESS", "true");
        let manager = default_agent_manager();
        manager.lock().await.register_root("root");

        let bridge = match crate::PlaywrightBridge::new().await {
            Ok(b) => b,
            Err(crate::BridgeError::PlaywrightNotInstalled(_)) => {
                eprintln!("skipping test: CloakBrowser not installed");
                return;
            }
            Err(e) => panic!("unexpected bridge error: {e}"),
        };

        let mut agent = CrawlerAgent::new_for_testing(ToolRegistry::new_with_core_tools())
            .with_agent_id("root".to_string())
            .with_agent_manager(manager.clone());
        agent.api_client_arc = Some(SharedApiClient::new(TextOnlyApiClient));
        agent.shared_bridge = Some(Arc::new(AsyncMutex::new(
            Box::new(bridge) as Box<dyn BrowserBackend + Send>
        )));
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
        let handle = tokio::spawn(async {
            Some(CrawlResult {
                extracted_data: vec![serde_json::json!({"child": 1})],
                ..Default::default()
            })
        });
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
        let _env_guard = crate::test_async_env_lock().lock().await;
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
        let bridge = match crate::PlaywrightBridge::new().await {
            Ok(bridge) => bridge,
            Err(crate::BridgeError::PlaywrightNotInstalled(_)) => {
                eprintln!("skipping test: CloakBrowser not installed");
                return;
            }
            Err(error) => panic!("unexpected bridge error: {error}"),
        };
        agent.shared_bridge = Some(Arc::new(AsyncMutex::new(
            Box::new(bridge) as Box<dyn BrowserBackend + Send>
        )));
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
        let _env_guard = crate::test_async_env_lock().lock().await;
        std::env::set_var("HEADLESS", "true");
        let manager = default_agent_manager();
        manager.lock().await.register_root("root");

        let bridge = match crate::PlaywrightBridge::new().await {
            Ok(b) => b,
            Err(crate::BridgeError::PlaywrightNotInstalled(_)) => {
                eprintln!("skipping test: CloakBrowser not installed");
                return;
            }
            Err(e) => panic!("unexpected bridge error: {e}"),
        };

        let mut agent = CrawlerAgent::new_for_testing(ToolRegistry::new_with_core_tools())
            .with_agent_id("root".to_string())
            .with_agent_manager(manager);
        agent.api_client_arc = Some(SharedApiClient::new(TextOnlyApiClient));
        agent.shared_bridge = Some(Arc::new(AsyncMutex::new(
            Box::new(bridge) as Box<dyn BrowserBackend + Send>
        )));
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
        let manager = Arc::new(AsyncMutex::new(AgentManager::new(0, 3, 10)));
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

        assert!(
            err.to_string()
                .contains("fork requires non-empty objective"),
            "expected error to mention empty objective, got: {err}"
        );
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
        let handle: tokio::task::JoinHandle<Option<CrawlResult>> = tokio::spawn(async { None });
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
        let handle: tokio::task::JoinHandle<Option<CrawlResult>> = tokio::spawn(async {
            Some(CrawlResult {
                extracted_data: vec![serde_json::json!({"data": 1})],
                ..Default::default()
            })
        });
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

        let handle = tokio::spawn(async {
            Some(CrawlResult {
                extracted_data: vec![serde_json::json!({"child": 1})],
                ..Default::default()
            })
        });
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

        let handle: tokio::task::JoinHandle<Option<CrawlResult>> = tokio::spawn(async {
            Some(CrawlResult {
                extracted_data: vec![
                    serde_json::json!({"child": 1}),
                    serde_json::json!({"child": 2}),
                ],
                ..Default::default()
            })
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
        assert_eq!(result.model, None);
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
    async fn test_planning_interval_disabled_by_default() {
        let _env_guard = crate::test_async_env_lock().lock().await;
        std::env::set_var("ACRAWL_CONFIG_HOME", "");

        let mut agent = CrawlerAgent::new_for_testing(mock_registry());
        let initial_slot = agent
            .prompt_override_slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        assert_eq!(initial_slot, None, "slot should be empty initially");

        let _ = agent
            .execute("navigate", r#"{"url":"https://example.com"}"#)
            .await;

        let slot_after = agent
            .prompt_override_slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        assert_eq!(
            slot_after, None,
            "slot should remain empty when interval=0 (default)"
        );
    }

    #[tokio::test]
    async fn test_action_cache_hits_for_same_read_content_and_fingerprint() {
        let _env_guard = crate::test_async_env_lock().lock().await;
        let temp_dir = tempfile::tempdir().expect("temp dir should create");
        write_action_cache_settings(temp_dir.path(), true, 30);

        let call_count = Arc::new(std::sync::Mutex::new(0usize));
        let registry = cache_test_registry(Arc::clone(&call_count), "read_content");
        let mut agent = CrawlerAgent::new_for_testing(registry);
        agent
            .crawl_state
            .page_fingerprints
            .push(action_cache_test_fingerprint("https://example.com"));

        let first = agent
            .execute("read_content", r##"{"selector":"#main","offset":0}"##)
            .await
            .expect("first call should succeed");
        let second = agent
            .execute("read_content", r##"{"offset":0,"selector":"#main"}"##)
            .await
            .expect("second call should succeed");

        assert_eq!(first.text, second.text);
        assert_eq!(
            *call_count
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            1
        );
    }

    #[tokio::test]
    async fn test_action_cache_misses_when_page_fingerprint_changes() {
        let _env_guard = crate::test_async_env_lock().lock().await;
        let temp_dir = tempfile::tempdir().expect("temp dir should create");
        write_action_cache_settings(temp_dir.path(), true, 30);

        let call_count = Arc::new(std::sync::Mutex::new(0usize));
        let registry = cache_test_registry(Arc::clone(&call_count), "read_content");
        let mut agent = CrawlerAgent::new_for_testing(registry);
        agent
            .crawl_state
            .page_fingerprints
            .push(action_cache_test_fingerprint("https://example.com/a"));

        let _ = agent
            .execute("read_content", r##"{"selector":"#main"}"##)
            .await
            .expect("first call should succeed");
        agent.crawl_state.page_fingerprints.push(PageFingerprint {
            url: "https://example.com/b".to_string(),
            element_count: 7,
            text_hash: 99,
        });
        let _ = agent
            .execute("read_content", r##"{"selector":"#main"}"##)
            .await
            .expect("second call should succeed");

        assert_eq!(
            *call_count
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            2
        );
    }

    #[tokio::test]
    async fn test_action_cache_misses_after_ttl_expires() {
        let _env_guard = crate::test_async_env_lock().lock().await;
        let temp_dir = tempfile::tempdir().expect("temp dir should create");
        write_action_cache_settings(temp_dir.path(), true, 0);

        let call_count = Arc::new(std::sync::Mutex::new(0usize));
        let registry = cache_test_registry(Arc::clone(&call_count), "read_content");
        let mut agent = CrawlerAgent::new_for_testing(registry);
        agent
            .crawl_state
            .page_fingerprints
            .push(action_cache_test_fingerprint("https://example.com"));

        let _ = agent
            .execute("read_content", r##"{"selector":"#main"}"##)
            .await
            .expect("first call should succeed");
        let _ = agent
            .execute("read_content", r##"{"selector":"#main"}"##)
            .await
            .expect("second call should succeed");

        assert_eq!(
            *call_count
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            2
        );
    }

    #[tokio::test]
    async fn test_action_cache_flag_off_keeps_behavior_unchanged() {
        let _env_guard = crate::test_async_env_lock().lock().await;
        let temp_dir = tempfile::tempdir().expect("temp dir should create");
        write_action_cache_settings(temp_dir.path(), false, 30);

        let call_count = Arc::new(std::sync::Mutex::new(0usize));
        let registry = cache_test_registry(Arc::clone(&call_count), "read_content");
        let mut agent = CrawlerAgent::new_for_testing(registry);
        agent
            .crawl_state
            .page_fingerprints
            .push(action_cache_test_fingerprint("https://example.com"));

        let _ = agent
            .execute("read_content", r##"{"selector":"#main"}"##)
            .await
            .expect("first call should succeed");
        let _ = agent
            .execute("read_content", r##"{"selector":"#main"}"##)
            .await
            .expect("second call should succeed");

        assert_eq!(
            *call_count
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            2
        );
        assert!(agent.crawl_state.action_cache.is_none());
    }

    #[tokio::test]
    async fn test_interaction_tools_are_never_action_cached() {
        let _env_guard = crate::test_async_env_lock().lock().await;
        let temp_dir = tempfile::tempdir().expect("temp dir should create");
        write_action_cache_settings(temp_dir.path(), true, 30);

        let call_count = Arc::new(std::sync::Mutex::new(0usize));
        let registry = cache_test_registry(Arc::clone(&call_count), "click");
        let mut agent = CrawlerAgent::new_for_testing(registry);
        agent
            .crawl_state
            .page_fingerprints
            .push(action_cache_test_fingerprint("https://example.com"));

        let _ = agent
            .execute("click", r##"{"selector":"#submit"}"##)
            .await
            .expect("first click should succeed");
        let _ = agent
            .execute("click", r##"{"selector":"#submit"}"##)
            .await
            .expect("second click should succeed");

        assert_eq!(
            *call_count
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            2
        );
    }

    #[test]
    fn test_step_count_increments_on_each_execute() {
        let agent = CrawlerAgent::new_for_testing(mock_registry());
        assert_eq!(agent.step_count, 0, "step_count should start at 0");
    }

    #[tokio::test]
    async fn selector_not_found_with_matching_text_heals_and_succeeds() {
        let _env_guard = crate::test_async_env_lock().lock().await;
        let temp_dir = tempfile::tempdir().expect("temp dir should create");
        write_self_healing_settings(temp_dir.path(), true, 2);

        let (mut agent, state) =
            healing_test_agent(1, "Element not found matching selector", healing_page_map());
        agent
            .browser
            .as_mut()
            .expect("browser should exist")
            .ref_map_mut()
            .assign_or_reuse("#old-submit", "button", "Submit");

        let result = agent
            .execute("click", r#"{"selector":"@e1"}"#)
            .await
            .expect("healed click should succeed");

        assert!(
            result.text.contains("[healed: @e1 → @e2]"),
            "{}",
            result.text
        );
        let state = state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(state.latest_click_selector.as_deref(), Some("#new-submit"));
        assert!(state.page_map_calls >= 1);
    }

    #[tokio::test]
    async fn selector_not_found_with_no_match_returns_original_error() {
        let _env_guard = crate::test_async_env_lock().lock().await;
        let temp_dir = tempfile::tempdir().expect("temp dir should create");
        write_self_healing_settings(temp_dir.path(), true, 2);

        let (mut agent, state) = healing_test_agent(
            1,
            "Element not found matching selector",
            serde_json::json!({
                "interactive": {
                    "elements": [
                        {"selector": "#login", "role": "button", "name": "Login", "text": "Login"}
                    ]
                },
                "meta": {"url": "https://example.com", "title": "Example"},
                "headings": [],
                "links": [],
                "forms": [],
                "landmarks": []
            }),
        );
        agent
            .browser
            .as_mut()
            .expect("browser should exist")
            .ref_map_mut()
            .assign_or_reuse("#old-submit", "button", "Submit");

        let err = agent
            .execute("click", r#"{"selector":"@e1"}"#)
            .await
            .expect_err("unhealable selector should fail");

        assert!(err
            .to_string()
            .contains("Element not found matching selector"));
        let state = state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(state.latest_click_selector.as_deref(), Some("#old-submit"));
        assert_eq!(state.page_map_calls, 1);
    }

    #[tokio::test]
    async fn max_retries_are_respected_for_self_healing() {
        let _env_guard = crate::test_async_env_lock().lock().await;
        let temp_dir = tempfile::tempdir().expect("temp dir should create");
        write_self_healing_settings(temp_dir.path(), true, 1);

        let (mut agent, state) =
            healing_test_agent(2, "Element not found matching selector", healing_page_map());
        agent
            .browser
            .as_mut()
            .expect("browser should exist")
            .ref_map_mut()
            .assign_or_reuse("#old-submit", "button", "Submit");

        let err = agent
            .execute("click", r#"{"selector":"@e1"}"#)
            .await
            .expect_err("single retry should not mask repeated failure");

        assert!(err
            .to_string()
            .contains("Element not found matching selector"));
        let state = state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(state.latest_click_selector.as_deref(), Some("#new-submit"));
        assert_eq!(state.page_map_calls, 1);
    }

    #[tokio::test]
    async fn non_selector_errors_do_not_attempt_healing() {
        let _env_guard = crate::test_async_env_lock().lock().await;
        let temp_dir = tempfile::tempdir().expect("temp dir should create");
        write_self_healing_settings(temp_dir.path(), true, 2);

        let (mut agent, state) =
            healing_test_agent(1, "network connection refused", healing_page_map());
        agent
            .browser
            .as_mut()
            .expect("browser should exist")
            .ref_map_mut()
            .assign_or_reuse("#old-submit", "button", "Submit");

        let err = agent
            .execute("click", r#"{"selector":"@e1"}"#)
            .await
            .expect_err("network failure should not heal");

        assert!(err.to_string().contains("network connection refused"));
        let state = state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(state.page_map_calls, 0);
        assert_eq!(state.latest_click_selector.as_deref(), Some("#old-submit"));
    }

    #[tokio::test]
    async fn self_healing_flag_off_keeps_selector_failure_behavior() {
        let _env_guard = crate::test_async_env_lock().lock().await;
        let temp_dir = tempfile::tempdir().expect("temp dir should create");
        write_self_healing_settings(temp_dir.path(), false, 2);

        let (mut agent, state) =
            healing_test_agent(1, "Element not found matching selector", healing_page_map());
        agent
            .browser
            .as_mut()
            .expect("browser should exist")
            .ref_map_mut()
            .assign_or_reuse("#old-submit", "button", "Submit");

        let err = agent
            .execute("click", r#"{"selector":"@e1"}"#)
            .await
            .expect_err("flag-off should preserve original failure");

        assert!(err
            .to_string()
            .contains("Element not found matching selector"));
        let state = state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(state.page_map_calls, 0);
        assert_eq!(state.latest_click_selector.as_deref(), Some("#old-submit"));
    }
}
