#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickerModelInfo {
    pub id: String,
    pub display_name: Option<String>,
}

use acrawl_core::message::ConversationMessage;
use agent::ChildEvent;
use api::provider::ModelInfo;

/// UI updates from the LLM stream, tool executor, or worker thread.
#[derive(Debug)]
pub enum ReplTuiEvent {
    /// Assistant or tool output as a raw markdown delta. The TUI typewriter
    /// renders this through `markdown::render_lines`; the non-TUI stdout
    /// sink feeds it into `MarkdownStreamState`.
    StreamText(String),
    TurnStarting,
    /// `Ok` when the model turn finished; `Err` is a user-visible error string.
    TurnFinished(Result<(), String>),
    SystemMessage(String),
    /// Notification that a tool call has started 閳?creates a transcript entry in TUI mode.
    ToolCallStart {
        name: String,
        input: String,
    },
    /// Notification that a tool call completed 閳?updates the transcript entry in TUI mode.
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
    /// Empty Vec means fetch failed 閳?caller falls back to builtin catalog.
    ModelCatalogReady(Vec<ModelInfo>),
    /// Extension bridge connection attempt finished.
    ExtensionBridgeResult {
        success: bool,
        message: String,
    },
    /// The runtime has entered the paused state.
    PauseStarted(String),
    /// The runtime has exited the paused state.
    PauseEnded,
    /// Event streamed from a forked child agent.
    #[allow(dead_code)]
    ChildEvent(ChildEvent),
    /// A message has been completed and is ready for display.
    MessageCompleted(ConversationMessage),
    /// A batch of messages has been loaded from storage or session.
    MessagesLoaded(Vec<ConversationMessage>),
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

    #[test]
    fn message_events_constructible() {
        let msg = ConversationMessage::user_text("hi");
        let _completed = ReplTuiEvent::MessageCompleted(msg.clone());
        let _loaded = ReplTuiEvent::MessagesLoaded(vec![msg]);
    }
}
