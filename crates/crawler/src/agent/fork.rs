use tokio::time::{Duration, Instant};

use runtime::ToolError;

use super::CrawlerAgent;
use crate::state::ChildBlock;
use crate::{
    tool_effect::{CancelSpec, ForkSpec, StatusSpec, WaitSpec},
    BrowserContext, ToolRegistry,
};

fn empty_wait_snapshot() -> String {
    serde_json::json!({
        "waited": 0,
        "finished": [],
        "still_running": [],
    })
    .to_string()
}

fn running_snapshot(child_id: &str, sub_goal: &str) -> serde_json::Value {
    serde_json::json!({
        "child_id": child_id,
        "sub_goal": sub_goal,
        "status": "running",
    })
}

fn finished_snapshot(
    child_id: &str,
    sub_goal: &str,
    success: bool,
    items_extracted: usize,
    error: Option<&str>,
) -> serde_json::Value {
    serde_json::json!({
        "child_id": child_id,
        "sub_goal": sub_goal,
        "status": if success { "completed" } else { "failed" },
        "success": success,
        "items_extracted": items_extracted,
        "error": error,
    })
}

pub(crate) struct ForkSupervisor<'a> {
    agent: &'a mut CrawlerAgent,
}

impl<'a> ForkSupervisor<'a> {
    fn new(agent: &'a mut CrawlerAgent) -> Self {
        Self { agent }
    }

    fn next_child_id(&self) -> String {
        // Monotonic per-agent counter: using `child_tasks.len() + 1` reused IDs
        // once children were drained by `wait_for_subagents`, which then made
        // every downstream lookup (control registry, manager, child_tasks)
        // ambiguous between current and prior generations.
        let n = self
            .agent
            .child_id_counter
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            + 1;
        format!("{}-child-{n}", self.agent.agent_id)
    }

    async fn claim_child_slot(&mut self, child_id: &str) -> Result<(), ToolError> {
        let settings = runtime::load_settings();
        let mut manager = self.agent.agent_manager.lock().await;
        if !manager.contains(&self.agent.agent_id) {
            manager.max_concurrent_per_parent =
                runtime::settings_get_max_concurrent_per_parent(&settings) as usize;
            manager.max_depth = runtime::settings_get_max_fork_depth(&settings) as usize;
            manager.max_total = runtime::settings_get_max_total_agents(&settings) as usize;
            manager.register_root(self.agent.agent_id.clone());
        }

        manager
            .register_child(child_id.to_string(), &self.agent.agent_id, None)
            .map_err(|error| ToolError::new(error.to_string()))
    }

    async fn release_child_slot(&mut self, child_id: &str) {
        self.agent.agent_manager.lock().await.mark_done(child_id);
    }
}

impl CrawlerAgent {
    pub(super) async fn handle_spawn(&mut self, fork_spec: ForkSpec) -> Result<String, ToolError> {
        let child_max_steps =
            runtime::settings_get_fork_child_max_steps(&runtime::load_settings()) as usize;
        let child_id = ForkSupervisor::new(self).next_child_id();
        ForkSupervisor::new(self)
            .claim_child_slot(&child_id)
            .await?;

        let setup = async {
            self.ensure_browser().await?;

            let child_state = self.crawl_state.fork(
                &fork_spec.goal,
                self.crawl_state.current_url.as_deref(),
                child_max_steps,
            );
            let child_api_client = self
                .api_client_arc
                .clone()
                .ok_or_else(|| ToolError::new("fork: api_client not initialized"))?;
            let shared_bridge = self
                .shared_bridge
                .clone()
                .ok_or_else(|| ToolError::new("fork: browser bridge not initialized"))?;
            let target_url = self.crawl_state.current_url.clone();
            let page_index = self
                .create_child_page(shared_bridge.clone(), target_url.as_deref())
                .await?;

            Ok::<_, ToolError>((child_state, child_api_client, shared_bridge, page_index))
        }
        .await;

        let (child_state, child_api_client, shared_bridge, page_index) = match setup {
            Ok(values) => values,
            Err(error) => {
                ForkSupervisor::new(self)
                    .release_child_slot(&child_id)
                    .await;
                return Err(error);
            }
        };

        // Register a fresh snapshot for this child. Children inherit the
        // parent's snapshot registry Arc so the parent can poll their
        // progress without joining.
        self.child_snapshots
            .register(&child_id, &fork_spec.goal, child_max_steps);

        let mut child_agent = CrawlerAgent::new(
            BrowserContext::new_shared(shared_bridge.clone(), page_index),
            ToolRegistry::new_with_core_tools(),
        )
        .with_max_steps(child_max_steps)
        .with_agent_id(child_id.clone())
        .with_agent_manager(self.agent_manager.clone())
        .with_child_snapshots(self.child_snapshots.clone())
        .with_control_state({
            if let Some(registry) = &self.child_control_registry {
                registry.register(&child_id)
            } else {
                self.control_state
                    .clone()
                    .unwrap_or_else(|| std::sync::Arc::new(runtime::ControlState::default()))
            }
        });
        if let Some(ref tx) = self.child_event_tx {
            child_agent = child_agent.with_child_event_sender(tx.clone());
        }
        if let Some(ref registry) = self.child_control_registry {
            child_agent = child_agent.with_child_control_registry(registry.clone());
        }
        child_agent.shared_bridge = Some(shared_bridge);
        child_agent.crawl_state = child_state;
        child_agent.api_client_arc = Some(child_api_client.clone());

        let fork_spec = ForkSpec {
            page_index: Some(page_index),
            ..fork_spec
        };
        let child_sub_goal = fork_spec.goal.clone();
        let runtime_handle = tokio::runtime::Handle::current();
        let join_handle = tokio::task::spawn_blocking(move || {
            runtime_handle
                .block_on(child_agent.run(&child_sub_goal, child_api_client))
                .ok()
                .map(|crawl_result| crawl_result.extracted_data)
        });
        self.child_tasks
            .insert(child_id.clone(), (fork_spec.goal.clone(), join_handle));

        let observation = format!("Forked subagent {child_id} for: {}", fork_spec.goal);
        self.crawl_state.action_history.push(observation.clone());
        Ok(observation)
    }

    pub(super) async fn handle_wait_effect(
        &mut self,
        wait_spec: WaitSpec,
    ) -> Result<String, ToolError> {
        let wait_timeout_secs = u64::from(runtime::settings_get_fork_wait_timeout_secs(
            &runtime::load_settings(),
        ));
        self.handle_wait_effect_with_timeout(wait_spec, Duration::from_secs(wait_timeout_secs))
            .await
    }

    // Wait collects results from finished children and reports back a typed
    // snapshot for children that haven't finished by the deadline. The wait
    // **never** aborts or cancels children: the deadline only controls how
    // long the parent blocks before returning. Cancellation is an explicit
    // action, surfaced via `cancel_subagent`.
    #[allow(clippy::too_many_lines)]
    async fn handle_wait_effect_with_timeout(
        &mut self,
        wait_spec: WaitSpec,
        wait_timeout: Duration,
    ) -> Result<String, ToolError> {
        let child_ids = wait_spec
            .child_ids
            .unwrap_or_else(|| self.child_tasks.keys().cloned().collect());
        if child_ids.is_empty() {
            return Ok(empty_wait_snapshot());
        }

        let deadline = Instant::now() + wait_timeout;
        let tasks = child_ids
            .into_iter()
            .filter_map(|child_id| {
                self.child_tasks
                    .remove(&child_id)
                    .map(|(sub_goal, handle)| (child_id, sub_goal, handle))
            })
            .collect::<Vec<_>>();

        if tasks.is_empty() {
            return Ok(empty_wait_snapshot());
        }

        let task_count = tasks.len();
        let mut finished_entries: Vec<serde_json::Value> = Vec::new();
        let mut still_running_entries: Vec<serde_json::Value> = Vec::new();

        for (child_id, sub_goal, mut handle) in tasks {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .unwrap_or_default();

            // Zero remaining is identical to "timed out": never abort, just
            // re-insert the handle so a future wait/cancel can still target
            // it, and report the child as still running.
            if remaining.is_zero() {
                still_running_entries.push(running_snapshot(&child_id, &sub_goal));
                self.child_tasks.insert(child_id, (sub_goal, handle));
                continue;
            }

            match tokio::time::timeout(remaining, &mut handle).await {
                Ok(Ok(Some(items))) => {
                    let item_count = items.len();
                    finished_entries.push(finished_snapshot(
                        &child_id, &sub_goal, true, item_count, None,
                    ));
                    self.emit_finished_event(&child_id, &sub_goal, true, item_count, None);
                    self.child_snapshots.update_with(&child_id, |snapshot| {
                        snapshot.state = crate::child_events::ChildLifecycle::Completed;
                        snapshot.items_extracted = item_count;
                    });
                    self.crawl_state.child_blocks.push(ChildBlock {
                        child_id: child_id.clone(),
                        sub_goal,
                        items,
                    });
                    self.cleanup_finished(&child_id).await;
                }
                Ok(Ok(None)) => {
                    finished_entries.push(finished_snapshot(
                        &child_id,
                        &sub_goal,
                        false,
                        0,
                        Some("child failed"),
                    ));
                    self.emit_finished_event(
                        &child_id,
                        &sub_goal,
                        false,
                        0,
                        Some("child failed".to_string()),
                    );
                    self.child_snapshots.update_with(&child_id, |snapshot| {
                        snapshot.state = crate::child_events::ChildLifecycle::Failed;
                        snapshot
                            .error
                            .get_or_insert_with(|| "child failed".to_string());
                    });
                    self.crawl_state.child_blocks.push(ChildBlock {
                        child_id: child_id.clone(),
                        sub_goal,
                        items: Vec::new(),
                    });
                    self.cleanup_finished(&child_id).await;
                }
                Ok(Err(error)) => {
                    let message = error.to_string();
                    finished_entries.push(finished_snapshot(
                        &child_id,
                        &sub_goal,
                        false,
                        0,
                        Some(&message),
                    ));
                    self.emit_finished_event(&child_id, &sub_goal, false, 0, Some(message.clone()));
                    self.child_snapshots.update_with(&child_id, |snapshot| {
                        snapshot.state = crate::child_events::ChildLifecycle::Failed;
                        snapshot.error = Some(message);
                    });
                    self.crawl_state.child_blocks.push(ChildBlock {
                        child_id: child_id.clone(),
                        sub_goal,
                        items: Vec::new(),
                    });
                    self.cleanup_finished(&child_id).await;
                }
                Err(_) => {
                    // Deadline elapsed mid-await. The child is still running;
                    // re-insert the handle and surface a "running" snapshot.
                    // No cancellation is performed.
                    still_running_entries.push(running_snapshot(&child_id, &sub_goal));
                    self.child_tasks.insert(child_id, (sub_goal, handle));
                }
            }
        }

        let finished_count = finished_entries.len();
        let still_running_count = still_running_entries.len();
        let total_items: u64 = finished_entries
            .iter()
            .map(|entry| {
                entry
                    .get("items_extracted")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0)
            })
            .sum();

        let payload = serde_json::json!({
            "waited": task_count,
            "finished": finished_entries,
            "still_running": still_running_entries,
        });

        let message = format!(
            "Waited on {task_count} subagent(s): {finished_count} finished, {still_running_count} still running. Collected {total_items} item(s)."
        );
        self.crawl_state.action_history.push(message);

        Ok(payload.to_string())
    }

    /// Cancel running sub-agents abortively. Each requested child's control
    /// state has `request_cancel()` called (so a cooperatively-yielding child
    /// observes cancellation between turns) and its `JoinHandle` is aborted
    /// immediately. The user-facing contract is "cancel = abort": discarded
    /// in-flight work is not collected.
    pub(super) async fn handle_cancel_effect(
        &mut self,
        spec: CancelSpec,
    ) -> Result<String, ToolError> {
        let mut cancelled: Vec<serde_json::Value> = Vec::new();
        let mut not_found: Vec<String> = Vec::new();

        for child_id in spec.child_ids {
            if let Some(registry) = &self.child_control_registry {
                if let Some(state) = registry.get(&child_id) {
                    state.request_cancel();
                }
            }

            match self.child_tasks.remove(&child_id) {
                Some((sub_goal, handle)) => {
                    handle.abort();
                    let reason = spec
                        .reason
                        .clone()
                        .unwrap_or_else(|| "cancelled by parent".to_string());
                    self.emit_finished_event(&child_id, &sub_goal, false, 0, Some(reason.clone()));
                    self.child_snapshots.mark_cancelled(&child_id, &reason);
                    self.crawl_state.child_blocks.push(ChildBlock {
                        child_id: child_id.clone(),
                        sub_goal: sub_goal.clone(),
                        items: Vec::new(),
                    });
                    cancelled.push(serde_json::json!({
                        "child_id": child_id,
                        "sub_goal": sub_goal,
                        "reason": reason,
                    }));
                    self.cleanup_finished(&child_id).await;
                }
                None => not_found.push(child_id),
            }
        }

        let cancelled_count = cancelled.len();
        let not_found_count = not_found.len();
        let payload = serde_json::json!({
            "cancelled": cancelled,
            "not_found": not_found,
        });
        let message =
            format!("Cancelled {cancelled_count} subagent(s); {not_found_count} not found.");
        self.crawl_state.action_history.push(message);
        Ok(payload.to_string())
    }

    /// Read-only status surface. Looks up the requested children (or all of
    /// them) in the snapshot registry and returns a JSON array of progress
    /// snapshots. Does NOT join, cancel, or otherwise mutate the running
    /// children — safe to call between any steps.
    #[allow(clippy::unused_async)]
    pub(super) async fn handle_status_effect(
        &mut self,
        spec: StatusSpec,
    ) -> Result<String, ToolError> {
        let snapshots: Vec<crate::child_events::ChildSnapshot> = match spec.child_ids {
            Some(ids) => ids
                .into_iter()
                .filter_map(|id| self.child_snapshots.get(&id))
                .collect(),
            None => self.child_snapshots.list(),
        };

        let now = std::time::Instant::now();
        let entries: Vec<serde_json::Value> = snapshots
            .into_iter()
            .map(|snapshot| {
                let last_event_secs_ago = now
                    .saturating_duration_since(snapshot.last_event_at)
                    .as_secs();
                serde_json::json!({
                    "child_id": snapshot.child_id,
                    "sub_goal": snapshot.sub_goal,
                    "state": snapshot.state.as_str(),
                    "step": snapshot.step,
                    "max_steps": snapshot.max_steps,
                    "last_tool": snapshot.last_tool,
                    "last_text": snapshot.last_text,
                    "items_extracted": snapshot.items_extracted,
                    "last_event_secs_ago": last_event_secs_ago,
                    "error": snapshot.error,
                })
            })
            .collect();
        Ok(serde_json::json!({ "children": entries }).to_string())
    }

    fn emit_finished_event(
        &self,
        child_id: &str,
        sub_goal: &str,
        success: bool,
        items_extracted: usize,
        error: Option<String>,
    ) {
        if let Some(ref tx) = self.child_event_tx {
            let _ = tx.send(crate::child_events::ChildEvent {
                child_id: child_id.to_string(),
                sub_goal: sub_goal.to_string(),
                event: crate::child_events::ChildEventKind::Finished {
                    success,
                    items_extracted,
                    error,
                },
            });
        }
    }

    async fn cleanup_finished(&self, child_id: &str) {
        if let Some(registry) = &self.child_control_registry {
            registry.remove(child_id);
        }
        self.agent_manager.lock().await.mark_done(child_id);
    }

    async fn create_child_page(
        &self,
        shared_bridge: crate::SharedBridge,
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use runtime::{ApiClient, ApiRequest, AssistantEvent, RuntimeError, ToolExecutor};
    use serde_json::Value;
    use tokio::sync::Mutex;

    use super::*;

    struct TextOnlyApiClient;

    impl ApiClient for TextOnlyApiClient {
        fn stream(&mut self, _request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
            Ok(vec![
                AssistantEvent::TextDelta("Child done.".to_string()),
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
                Ok(crate::ToolEffect::Reply(format!("Navigated to {url}")))
            }),
        );
        registry.register(
            "wait_for_subagents",
            Box::new(crate::tools::wait_for_subagents::execute),
        );
        registry
    }

    async fn test_bridge() -> crate::SharedBridge {
        Arc::new(Mutex::new(
            crate::PlaywrightBridge::new()
                .await
                .expect("bridge should initialize for fork test"),
        ))
    }

    #[tokio::test]
    async fn test_fork_spec_goal_is_preserved() {
        let manager = super::super::default_agent_manager();
        manager.lock().await.register_root("root");

        let mut agent = CrawlerAgent::new_for_testing(ToolRegistry::new_with_core_tools())
            .with_agent_id("root".to_string())
            .with_agent_manager(manager);
        agent.api_client_arc = Some(crate::SharedApiClient::new(TextOnlyApiClient));
        agent.shared_bridge = Some(test_bridge().await);
        agent.fork_page_index_override = Some(1);

        let observation = agent
            .execute("fork", r#"{"sub_goal":"collect details"}"#)
            .await
            .expect("fork should succeed");

        assert_eq!(
            observation,
            "Forked subagent root-child-1 for: collect details"
        );
        assert_eq!(
            agent.child_tasks.get("root-child-1").unwrap().0,
            "collect details"
        );
        for (_, (_, handle)) in agent.child_tasks.drain() {
            handle.abort();
        }
    }

    #[tokio::test]
    async fn test_fork_limit_not_exceeded() {
        let manager = super::super::default_agent_manager();
        manager.lock().await.register_root("root");

        let call_count = Arc::new(std::sync::Mutex::new(0usize));
        let mut agent = CrawlerAgent::new_for_testing(ToolRegistry::new_with_core_tools())
            .with_agent_id("root".to_string())
            .with_agent_manager(manager.clone());
        agent.api_client_arc = Some(crate::SharedApiClient::new(CountingTextOnlyApiClient {
            call_count: call_count.clone(),
        }));
        agent.shared_bridge = Some(test_bridge().await);
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
    async fn test_fork_cleanup_on_error() {
        let manager = super::super::default_agent_manager();
        manager.lock().await.register_root("root");

        let mut agent = CrawlerAgent::new_for_testing(ToolRegistry::new_with_core_tools())
            .with_agent_id("root".to_string())
            .with_agent_manager(manager.clone());
        agent.shared_bridge = Some(test_bridge().await);
        agent.fork_page_index_override = Some(1);

        let error = agent
            .execute("fork", r#"{"sub_goal":"collect details"}"#)
            .await
            .expect_err("fork should fail when api client is missing");

        assert!(error.to_string().contains("api_client not initialized"));
        let manager = manager.lock().await;
        let child = manager
            .get_children("root")
            .into_iter()
            .next()
            .expect("child slot should be tracked");
        assert_eq!(child.status, crate::AgentStatus::Done);
    }

    #[tokio::test]
    async fn test_wait_no_children_returns_empty_snapshot() {
        let mut agent = CrawlerAgent::new_for_testing(mock_registry());
        let result = agent
            .handle_wait_effect(WaitSpec { child_ids: None })
            .await
            .expect("wait with no children should succeed");
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["waited"], 0);
        assert!(parsed["finished"].as_array().unwrap().is_empty());
        assert!(parsed["still_running"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_wait_records_step() {
        let manager = super::super::default_agent_manager();
        manager.lock().await.register_root("test-agent");

        let mut agent = CrawlerAgent::new_for_testing(mock_registry()).with_agent_manager(manager);
        let handle: tokio::task::JoinHandle<Option<Vec<Value>>> = tokio::spawn(async { None });
        agent
            .child_tasks
            .insert("child-1".to_string(), ("search".to_string(), handle));

        let result = agent.execute("wait_for_subagents", "{}").await.unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["waited"], 1);
        // child returned None → counted as finished/failed, not still_running
        assert_eq!(parsed["finished"].as_array().unwrap().len(), 1);
        assert!(parsed["still_running"].as_array().unwrap().is_empty());
        assert_eq!(agent.crawl_state.action_history.len(), 1);
        assert!(agent.crawl_state.action_history[0].contains("Waited on 1 subagent(s)"));
    }

    #[tokio::test]
    async fn test_wait_filters_by_specific_child_ids() {
        let manager = super::super::default_agent_manager();
        manager.lock().await.register_root("test-agent");

        let mut agent = CrawlerAgent::new_for_testing(mock_registry()).with_agent_manager(manager);
        let handle1: tokio::task::JoinHandle<Option<Vec<Value>>> =
            tokio::spawn(async { Some(vec![serde_json::json!({"from": "child-1"})]) });
        let handle2: tokio::task::JoinHandle<Option<Vec<Value>>> =
            tokio::spawn(async { Some(vec![serde_json::json!({"from": "child-2"})]) });
        agent
            .child_tasks
            .insert("child-1".to_string(), ("goal-1".to_string(), handle1));
        agent
            .child_tasks
            .insert("child-2".to_string(), ("goal-2".to_string(), handle2));

        let result = agent
            .handle_wait_effect(WaitSpec {
                child_ids: Some(vec!["child-1".to_string()]),
            })
            .await
            .unwrap();

        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["waited"], 1);
        assert_eq!(agent.crawl_state.child_blocks.len(), 1);
        assert_eq!(agent.crawl_state.child_blocks[0].child_id, "child-1");
        assert!(agent.child_tasks.contains_key("child-2"));
        for (_, (_, handle)) in agent.child_tasks.drain() {
            handle.abort();
        }
    }

    #[tokio::test]
    async fn test_wait_with_nonexistent_ids_returns_empty_snapshot() {
        let mut agent = CrawlerAgent::new_for_testing(mock_registry());
        let handle: tokio::task::JoinHandle<Option<Vec<Value>>> =
            tokio::spawn(async { Some(vec![serde_json::json!({"data": 1})]) });
        agent
            .child_tasks
            .insert("real-child".to_string(), ("goal".to_string(), handle));

        let result = agent
            .handle_wait_effect(WaitSpec {
                child_ids: Some(vec!["nonexistent".to_string()]),
            })
            .await
            .unwrap();

        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["waited"], 0);
        assert!(agent.child_tasks.contains_key("real-child"));
        for (_, (_, handle)) in agent.child_tasks.drain() {
            handle.abort();
        }
    }

    #[tokio::test]
    async fn test_wait_collects_from_multiple_children() {
        let manager = super::super::default_agent_manager();
        manager.lock().await.register_root("test-agent");

        let mut agent = CrawlerAgent::new_for_testing(mock_registry()).with_agent_manager(manager);
        let handle1: tokio::task::JoinHandle<Option<Vec<Value>>> =
            tokio::spawn(async { Some(vec![serde_json::json!({"a": 1})]) });
        let handle2: tokio::task::JoinHandle<Option<Vec<Value>>> = tokio::spawn(async {
            Some(vec![
                serde_json::json!({"b": 2}),
                serde_json::json!({"c": 3}),
            ])
        });
        agent
            .child_tasks
            .insert("child-1".to_string(), ("goal-1".to_string(), handle1));
        agent
            .child_tasks
            .insert("child-2".to_string(), ("goal-2".to_string(), handle2));

        let result = agent
            .handle_wait_effect(WaitSpec { child_ids: None })
            .await
            .unwrap();

        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["waited"], 2);
        assert_eq!(parsed["finished"].as_array().unwrap().len(), 2);
        let total: u64 = parsed["finished"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["items_extracted"].as_u64().unwrap())
            .sum();
        assert_eq!(total, 3);
        assert_eq!(agent.crawl_state.child_blocks.len(), 2);
        assert!(agent.child_tasks.is_empty());
    }

    #[tokio::test]
    async fn test_wait_emits_finished_event_on_child_completion() {
        let manager = super::super::default_agent_manager();
        manager.lock().await.register_root("test-agent");

        let (tx, rx) = std::sync::mpsc::channel();
        let mut agent = CrawlerAgent::new_for_testing(mock_registry())
            .with_agent_manager(manager)
            .with_child_event_sender(tx);
        let handle: tokio::task::JoinHandle<Option<Vec<Value>>> =
            tokio::spawn(async { Some(vec![serde_json::json!({"a": 1})]) });
        agent
            .child_tasks
            .insert("child-1".to_string(), ("goal-1".to_string(), handle));

        let result = agent
            .handle_wait_effect(WaitSpec {
                child_ids: Some(vec!["child-1".to_string()]),
            })
            .await
            .unwrap();

        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["finished"][0]["items_extracted"], 1);
        let event = rx.try_recv().expect("finished event should be emitted");
        assert_eq!(event.child_id, "child-1");
        assert_eq!(event.sub_goal, "goal-1");
        assert!(matches!(
            event.event,
            crate::child_events::ChildEventKind::Finished {
                success: true,
                items_extracted: 1,
                error: None,
            }
        ));
    }

    #[tokio::test]
    async fn test_wait_returns_snapshot_on_timeout_without_aborting() {
        let manager = super::super::default_agent_manager();
        manager.lock().await.register_root("test-agent");

        let (tx, rx) = std::sync::mpsc::channel();
        let registry = crate::ChildControlRegistry::default();
        let child_state = registry.register("child-1");
        let mut agent = CrawlerAgent::new_for_testing(mock_registry())
            .with_agent_manager(manager)
            .with_child_event_sender(tx)
            .with_child_control_registry(registry.clone());
        let handle: tokio::task::JoinHandle<Option<Vec<Value>>> = tokio::spawn(async {
            tokio::time::sleep(Duration::from_secs(1)).await;
            Some(Vec::new())
        });
        agent
            .child_tasks
            .insert("child-1".to_string(), ("goal-1".to_string(), handle));

        let result = agent
            .handle_wait_effect_with_timeout(
                WaitSpec {
                    child_ids: Some(vec!["child-1".to_string()]),
                },
                Duration::ZERO,
            )
            .await
            .unwrap();

        // The child is still running, so the snapshot must report it under
        // `still_running` and NOT abort or cancel it.
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["waited"], 1);
        assert!(parsed["finished"].as_array().unwrap().is_empty());
        let still_running = parsed["still_running"].as_array().unwrap();
        assert_eq!(still_running.len(), 1);
        assert_eq!(still_running[0]["child_id"], "child-1");
        assert_eq!(still_running[0]["status"], "running");

        // No cancel was requested and no Finished event was emitted.
        assert!(!child_state.is_cancelled());
        assert!(
            rx.try_recv().is_err(),
            "no Finished event should be emitted on timeout"
        );

        // The handle is re-inserted so a future wait/cancel can still target it.
        assert!(agent.child_tasks.contains_key("child-1"));
        for (_, (_, handle)) in agent.child_tasks.drain() {
            handle.abort();
        }
    }

    #[tokio::test]
    async fn test_cancel_subagent_aborts_handle_and_emits_finished_event() {
        let manager = super::super::default_agent_manager();
        manager.lock().await.register_root("test-agent");

        let (tx, rx) = std::sync::mpsc::channel();
        let registry = crate::ChildControlRegistry::default();
        let child_state = registry.register("child-1");
        let mut agent = CrawlerAgent::new_for_testing(mock_registry())
            .with_agent_manager(manager)
            .with_child_event_sender(tx)
            .with_child_control_registry(registry.clone());
        let handle: tokio::task::JoinHandle<Option<Vec<Value>>> = tokio::spawn(async {
            tokio::time::sleep(Duration::from_secs(60)).await;
            Some(Vec::new())
        });
        agent
            .child_tasks
            .insert("child-1".to_string(), ("goal-1".to_string(), handle));

        let result = agent
            .handle_cancel_effect(crate::tool_effect::CancelSpec {
                child_ids: vec!["child-1".to_string()],
                reason: Some("user pressed stop".to_string()),
            })
            .await
            .unwrap();

        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["cancelled"].as_array().unwrap().len(), 1);
        assert_eq!(parsed["cancelled"][0]["child_id"], "child-1");
        assert_eq!(parsed["cancelled"][0]["reason"], "user pressed stop");
        assert!(parsed["not_found"].as_array().unwrap().is_empty());

        assert!(child_state.is_cancelled());
        assert!(!agent.child_tasks.contains_key("child-1"));
        let event = rx.try_recv().expect("Finished event should be emitted");
        assert_eq!(event.child_id, "child-1");
        assert!(matches!(
            event.event,
            crate::child_events::ChildEventKind::Finished {
                success: false,
                items_extracted: 0,
                error: Some(ref error),
            } if error == "user pressed stop"
        ));
    }

    #[tokio::test]
    async fn test_handle_status_effect_returns_snapshots() {
        let mut agent = CrawlerAgent::new_for_testing(mock_registry());
        agent
            .child_snapshots
            .register("child-1", "search page 2", 15);
        agent.child_snapshots.update_with("child-1", |snapshot| {
            snapshot.state = crate::child_events::ChildLifecycle::Running;
            snapshot.last_tool = Some("navigate".to_string());
            snapshot.step = 3;
        });

        let result = agent
            .handle_status_effect(crate::tool_effect::StatusSpec { child_ids: None })
            .await
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        let children = parsed["children"].as_array().unwrap();
        assert_eq!(children.len(), 1);
        assert_eq!(children[0]["child_id"], "child-1");
        assert_eq!(children[0]["state"], "running");
        assert_eq!(children[0]["last_tool"], "navigate");
        assert_eq!(children[0]["step"], 3);
        assert_eq!(children[0]["max_steps"], 15);
    }

    #[tokio::test]
    async fn test_handle_status_effect_filters_by_child_ids() {
        let mut agent = CrawlerAgent::new_for_testing(mock_registry());
        agent.child_snapshots.register("child-1", "g1", 10);
        agent.child_snapshots.register("child-2", "g2", 10);

        let result = agent
            .handle_status_effect(crate::tool_effect::StatusSpec {
                child_ids: Some(vec!["child-2".to_string(), "unknown".to_string()]),
            })
            .await
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        let children = parsed["children"].as_array().unwrap();
        assert_eq!(children.len(), 1);
        assert_eq!(children[0]["child_id"], "child-2");
    }

    #[tokio::test]
    async fn test_cancel_subagent_reports_unknown_child_ids() {
        let mut agent = CrawlerAgent::new_for_testing(mock_registry());
        let result = agent
            .handle_cancel_effect(crate::tool_effect::CancelSpec {
                child_ids: vec!["ghost".to_string()],
                reason: None,
            })
            .await
            .unwrap();

        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert!(parsed["cancelled"].as_array().unwrap().is_empty());
        assert_eq!(parsed["not_found"].as_array().unwrap().len(), 1);
        assert_eq!(parsed["not_found"][0], "ghost");
    }

    #[test]
    fn next_child_id_is_monotonic_across_drains() {
        // Even after the child_tasks map has been drained (which used to
        // reset the count to 0), subsequent IDs must keep increasing so that
        // every child gets a globally unique handle across the agent's life.
        let mut agent = CrawlerAgent::new_for_testing(ToolRegistry::new());

        let mut ids = std::collections::HashSet::new();
        for _ in 0..3 {
            ids.insert(ForkSupervisor::new(&mut agent).next_child_id());
        }
        // Simulate `wait_for_subagents` having drained completed children.
        agent.child_tasks.clear();
        for _ in 0..3 {
            ids.insert(ForkSupervisor::new(&mut agent).next_child_id());
        }
        assert_eq!(
            ids.len(),
            6,
            "next_child_id must produce unique IDs even after drains: {ids:?}"
        );
    }
}
