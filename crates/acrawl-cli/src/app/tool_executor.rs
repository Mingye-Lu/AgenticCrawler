use std::sync::Arc;

use crawler::{CrawlerAgent, SharedApiClient, ToolRegistry};
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
}

impl ToolExecutor for CliToolExecutor {
    async fn execute(&mut self, tool_name: &str, input: &str) -> Result<String, ToolError> {
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
