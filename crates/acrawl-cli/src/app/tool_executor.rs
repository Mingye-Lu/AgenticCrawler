use crawler::{CrawlerAgent, SharedApiClient, ToolRegistry};
use runtime::{ToolError, ToolExecutor};

use super::AllowedToolSet;

pub(crate) struct CliToolExecutor {
    allowed_tools: Option<AllowedToolSet>,
    agent: CrawlerAgent,
}

impl CliToolExecutor {
    pub(crate) fn new(allowed_tools: Option<AllowedToolSet>, fork_client: SharedApiClient) -> Self {
        Self {
            allowed_tools,
            agent: CrawlerAgent::new_lazy(ToolRegistry::new_with_core_tools())
                .with_api_client(fork_client),
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
