use std::sync::Arc;

use acrawl_core::ToolOutcome;
use agent::CrawlerAgent;
use agent::ToolRegistry;
use crawler::SharedApiClient;
use runtime::{ControlState, ToolError, ToolExecutor};

use super::AllowedToolSet;

pub(crate) struct CliToolExecutor {
    allowed_tools: Option<AllowedToolSet>,
    agent: CrawlerAgent,
}

impl CliToolExecutor {
    pub(crate) fn new(
        allowed_tools: Option<AllowedToolSet>,
        fork_client: SharedApiClient,
        is_interactive: bool,
        control_state: Option<Arc<ControlState>>,
        child_event_tx: Option<std::sync::mpsc::Sender<crawler::ChildEvent>>,
        child_control_registry: Option<crawler::ChildControlRegistry>,
    ) -> Self {
        let registry = ToolRegistry::new_with_options(is_interactive);
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

    pub(crate) fn reset_browser(&mut self) {
        self.agent.reset_browser();
    }

    pub(crate) fn clear_extension_bridge(&mut self) {
        self.agent.clear_shared_bridge();
    }

    pub(crate) fn set_extension_bridge(&mut self, bridge: crawler::SharedBridge) {
        self.agent.set_shared_bridge(bridge);
    }

    pub(crate) fn set_extension_mode(&mut self, active: bool) {
        self.agent.set_extension_mode(active);
    }

    pub(crate) async fn export_current_state(&mut self) -> Option<crawler::BrowserState> {
        self.agent.export_browser_state().await
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
