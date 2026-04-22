use crate::tui::model_list::ModelInfo as PickerModelInfo;
use api::provider::ModelInfo;

/// UI updates from the LLM stream, tool executor, or worker thread.
#[derive(Debug)]
pub enum ReplTuiEvent {
    /// Assistant or tool output as ANSI (parsed with ansi-to-tui).
    StreamAnsi(String),
    TurnStarting,
    /// `Ok` when the model turn finished; `Err` is a user-visible error string.
    TurnFinished(Result<(), String>),
    /// Notification that the AI has started executing a specific tool.
    ToolStarting {
        name: String,
        input: String,
    },
    SystemMessage(String),
    /// Notification that a tool call has started — creates a transcript entry in TUI mode.
    ToolCallStart {
        name: String,
        input: String,
    },
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
    /// Auth modal finished fetching provider-specific models in a background thread.
    AuthModelsLoaded(Result<Vec<PickerModelInfo>, String>),
    /// Live model catalog fetched from models.dev on REPL startup.
    /// Empty Vec means fetch failed — caller falls back to builtin catalog.
    ModelCatalogReady(Vec<ModelInfo>),
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

        let _auth_models_loaded = ReplTuiEvent::AuthModelsLoaded(Ok(Vec::new()));

        let _catalog_ready = ReplTuiEvent::ModelCatalogReady(Vec::new());
    }
}
