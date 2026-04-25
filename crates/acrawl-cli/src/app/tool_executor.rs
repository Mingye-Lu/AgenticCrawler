use std::io;

use crate::tui::tool_panel::format_tool_result;
use crawler::{CrawlerAgent, SharedApiClient, ToolRegistry};
use runtime::{ToolError, ToolExecutor};

use super::{AllowedToolSet, TerminalRenderer};
use crate::tui::ReplTuiEvent;

pub(crate) struct CliToolExecutor {
    renderer: TerminalRenderer,
    emit_output: bool,
    allowed_tools: Option<AllowedToolSet>,
    agent: CrawlerAgent,
    ui_tx: Option<std::sync::mpsc::Sender<ReplTuiEvent>>,
}

impl CliToolExecutor {
    pub(crate) fn new(
        allowed_tools: Option<AllowedToolSet>,
        emit_output: bool,
        ui_tx: Option<std::sync::mpsc::Sender<ReplTuiEvent>>,
        fork_client: SharedApiClient,
    ) -> Self {
        Self {
            renderer: TerminalRenderer::new(),
            emit_output,
            allowed_tools,
            agent: CrawlerAgent::new_lazy(ToolRegistry::new_with_core_tools())
                .with_api_client(fork_client),
            ui_tx,
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
        if let Some(tx) = &self.ui_tx {
            let _ = tx.send(ReplTuiEvent::ToolStarting {
                name: tool_name.to_string(),
                input: input.to_string(),
            });
        }

        match self.agent.execute(tool_name, input).await {
            Ok(output) => {
                if self.emit_output {
                    let markdown = format_tool_result(tool_name, &output, false);
                    self.renderer
                        .stream_markdown(&markdown, &mut io::stdout())
                        .map_err(|error: io::Error| ToolError::new(error.to_string()))?;
                } else if let Some(tx) = &self.ui_tx {
                    let _ = tx.send(ReplTuiEvent::ToolCallComplete {
                        name: tool_name.to_string(),
                        output: output.clone(),
                        is_error: false,
                    });
                }
                Ok(output)
            }
            Err(error) => {
                if self.emit_output {
                    let rendered_error = error.to_string();
                    let markdown = format_tool_result(tool_name, &rendered_error, true);
                    self.renderer
                        .stream_markdown(&markdown, &mut io::stdout())
                        .map_err(|stream_error: io::Error| ToolError::new(stream_error.to_string()))?;
                } else if let Some(tx) = &self.ui_tx {
                    let _ = tx.send(ReplTuiEvent::ToolCallComplete {
                        name: tool_name.to_string(),
                        output: error.to_string(),
                        is_error: true,
                    });
                }
                Err(error)
            }
        }
    }
}
