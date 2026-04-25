use tokio::time::{Duration, Instant};

use runtime::ToolError;

use super::CrawlerAgent;
use crate::state::ChildBlock;
use crate::{tool_effect::ForkSpec, tool_effect::WaitSpec, BrowserContext, ToolRegistry};

pub(crate) struct ForkSupervisor<'a> {
    agent: &'a mut CrawlerAgent,
}

impl<'a> ForkSupervisor<'a> {
    fn new(agent: &'a mut CrawlerAgent) -> Self {
        Self { agent }
    }

    fn next_child_id(&self) -> String {
        format!(
            "{}-child-{}",
            self.agent.agent_id,
            self.agent.child_tasks.len() + 1
        )
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
        let child_ids = wait_spec
            .child_ids
            .unwrap_or_else(|| self.child_tasks.keys().cloned().collect());
        if child_ids.is_empty() {
            return Ok("No active subagents".to_string());
        }

        let wait_timeout_secs = u64::from(runtime::settings_get_fork_wait_timeout_secs(
            &runtime::load_settings(),
        ));
        let deadline = Instant::now() + Duration::from_secs(wait_timeout_secs);
        let tasks = child_ids
            .into_iter()
            .filter_map(|child_id| {
                self.child_tasks
                    .remove(&child_id)
                    .map(|(sub_goal, handle)| (child_id, sub_goal, handle))
            })
            .collect::<Vec<_>>();

        if tasks.is_empty() {
            return Ok("No active subagents".to_string());
        }

        let task_count = tasks.len();
        let mut total_items = 0_usize;

        for (child_id, sub_goal, mut handle) in tasks {
            let remaining = deadline
                .checked_duration_since(Instant::now())
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

        let message =
            format!("Waited for {task_count} subagent(s). Collected {total_items} item(s).");
        self.crawl_state.action_history.push(message.clone());
        Ok(message)
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
        registry.register("done", Box::new(crate::tools::done::execute));
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
    async fn test_wait_no_children_returns_immediately() {
        let mut agent = CrawlerAgent::new_for_testing(mock_registry());
        let result = agent.handle_wait_effect(WaitSpec { child_ids: None }).await;
        assert_eq!(result.unwrap(), "No active subagents");
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
        assert_eq!(result, "Waited for 1 subagent(s). Collected 0 item(s).");
        assert_eq!(agent.crawl_state.action_history[0], result);
    }

    #[tokio::test]
    async fn test_done_auto_waits_for_children() {
        let manager = super::super::default_agent_manager();
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
        assert_eq!(agent.crawl_state.child_blocks.len(), 1);
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

        assert!(result.contains("Waited for 1 subagent(s)"));
        assert_eq!(agent.crawl_state.child_blocks.len(), 1);
        assert_eq!(agent.crawl_state.child_blocks[0].child_id, "child-1");
        assert!(agent.child_tasks.contains_key("child-2"));
        for (_, (_, handle)) in agent.child_tasks.drain() {
            handle.abort();
        }
    }

    #[tokio::test]
    async fn test_wait_with_nonexistent_ids_returns_no_active() {
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

        assert_eq!(result, "No active subagents");
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

        assert!(result.contains("Waited for 2 subagent(s)"));
        assert!(result.contains("Collected 3 item(s)"));
        assert_eq!(agent.crawl_state.child_blocks.len(), 2);
        assert!(agent.child_tasks.is_empty());
    }
}
