use std::sync::Arc;

use acrawl_core::ToolOutcome;
use agent::{ChildControlRegistry, ChildEvent, CrawlerAgent, SharedApiClient, ToolRegistry};
use browser::{BrowserState, SharedBridge};
use runtime::{ControlState, ToolError, ToolExecutor};

use super::AllowedToolSet;

pub struct CliToolExecutor {
    allowed_tools: Option<AllowedToolSet>,
    agent: CrawlerAgent,
}

impl CliToolExecutor {
    pub fn new(
        allowed_tools: Option<AllowedToolSet>,
        fork_client: SharedApiClient,
        _is_interactive: bool,
        control_state: Option<Arc<ControlState>>,
        child_event_tx: Option<std::sync::mpsc::Sender<ChildEvent>>,
        child_control_registry: Option<ChildControlRegistry>,
    ) -> Self {
        let registry = ToolRegistry::new_with_core_tools();
        let mut agent = CrawlerAgent::new_lazy(registry).with_api_client(fork_client);
        if let Some(state) = control_state {
            agent = agent.with_control_state(state);
        }
        if let Some(tx) = child_event_tx {
            agent = agent.with_child_event_sender(tx);
        }
        if let Some(reg) = child_control_registry {
            agent = agent.with_child_control_registry(reg);
        }
        Self {
            allowed_tools,
            agent,
        }
    }

    pub fn reset_browser(&mut self) {
        self.agent.reset_browser();
    }

    pub fn clear_extension_bridge(&mut self) {
        self.agent.clear_shared_bridge();
    }

    pub fn set_extension_bridge(&mut self, bridge: SharedBridge) {
        self.agent.set_shared_bridge(bridge);
    }

    pub fn set_extension_mode(&mut self, active: bool) {
        self.agent.set_extension_mode(active);
    }

    pub async fn export_current_state(&mut self) -> Option<BrowserState> {
        self.agent.export_browser_state().await
    }

    pub fn take_captured_child_sessions(&mut self) -> Vec<runtime::ChildSession> {
        self.agent.take_captured_child_sessions()
    }
}

impl ToolExecutor for CliToolExecutor {
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
                    "tool `{tool_name}` is not enabled by the current --allowedTools setting"
                )));
            }
            match self.agent.execute(tool_name, input).await {
                Ok(output) => Ok(output),
                Err(error) => Err(error),
            }
        }
    }
}
