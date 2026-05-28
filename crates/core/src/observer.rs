use crate::{message::ConversationMessage, message::TokenUsage, ToolEffect};

/// Observer that receives events from `ConversationRuntime`.
/// All methods have default no-op implementations.
pub trait RuntimeObserver: Send {
    fn on_text_delta(&mut self, text: &str) {
        let _ = text;
    }

    fn on_pause_started(&mut self, reason: &str) {
        let _ = reason;
    }

    fn on_pause_ended(&mut self) {}

    fn on_tool_call_start(&mut self, id: &str, name: &str, input: &str) {
        let _ = (id, name, input);
    }

    fn on_tool_result(&mut self, name: &str, output: &str, is_error: bool) {
        let _ = (name, output, is_error);
    }

    fn on_tool_effect(&self, _effect: &ToolEffect) {}

    fn on_system_message(&mut self, msg: &str) {
        let _ = msg;
    }

    fn on_turn_finished(&mut self, result: &Result<(), String>) {
        let _ = result;
    }

    fn on_usage(&mut self, usage: &TokenUsage) {
        let _ = usage;
    }

    fn on_message_completed(&mut self, msg: &ConversationMessage) {
        let _ = msg;
    }
}
