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
    #[allow(dead_code)]
    /// Notification that a tool call has started — creates a transcript entry in TUI mode.
    ToolCallStart {
        name: String,
        input: String,
    },
    #[allow(dead_code)]
    /// Notification that a tool call completed — updates the transcript entry in TUI mode.
    ToolCallComplete {
        name: String,
        output: String,
        is_error: bool,
    },
    /// OAuth flow finished (success or error).
    AuthOAuthComplete {
        provider: String,
        result: Result<(), String>,
    },
    /// Status update from OAuth thread (e.g., "Listening on port 4545...").
    AuthOAuthProgress {
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_event_variants_constructible() {
        // Verify AuthOAuthComplete can be constructed
        let _complete_ok = ReplTuiEvent::AuthOAuthComplete {
            provider: "anthropic".to_string(),
            result: Ok(()),
        };

        let _complete_err = ReplTuiEvent::AuthOAuthComplete {
            provider: "openai".to_string(),
            result: Err("auth failed".to_string()),
        };

        // Verify AuthOAuthProgress can be constructed
        let _progress = ReplTuiEvent::AuthOAuthProgress {
            message: "Listening on port 4545...".to_string(),
        };
    }
}
