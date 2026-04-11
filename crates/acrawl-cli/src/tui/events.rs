use runtime::{PermissionPromptDecision, PermissionRequest};
use std::sync::mpsc::Sender;

/// UI updates from the LLM stream, tool executor, or worker thread.
#[derive(Debug)]
pub enum ReplTuiEvent {
    /// Assistant or tool output as ANSI (parsed with ansi-to-tui).
    StreamAnsi(String),
    TurnStarting,
    /// `Ok` when the model turn finished; `Err` is a user-visible error string.
    TurnFinished(Result<(), String>),
    PermissionNeeded {
        request: PermissionRequest,
        respond: Sender<PermissionPromptDecision>,
    },
    /// Notification that the AI has started executing a specific tool.
    ToolStarting {
        name: String,
        input: String,
    },
    SystemMessage(String),
}
