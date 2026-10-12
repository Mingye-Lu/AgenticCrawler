//! `ChannelSink` — bridges runtime events to the TUI via channel.

use std::sync::mpsc;

use render::sink::OutputSink;
use runtime::ConversationMessage;

use crate::events::ReplTuiEvent;

#[derive(Debug)]
pub struct ChannelSink {
    tx: mpsc::Sender<ReplTuiEvent>,
}

impl ChannelSink {
    #[must_use]
    pub fn new(tx: mpsc::Sender<ReplTuiEvent>) -> Self {
        Self { tx }
    }
}

impl OutputSink for ChannelSink {
    fn on_text_delta(&mut self, raw_text: &str) {
        let _ = self.tx.send(ReplTuiEvent::StreamText(raw_text.to_string()));
    }

    fn on_tool_call(&mut self, name: &str, input: &str) {
        let _ = self.tx.send(ReplTuiEvent::ToolCallStart {
            name: name.to_string(),
            input: input.to_string(),
        });
    }

    fn on_tool_result(&mut self, name: &str, output: &str, is_error: bool) {
        let _ = self.tx.send(ReplTuiEvent::ToolCallComplete {
            name: name.to_string(),
            output: output.to_string(),
            is_error,
        });
    }

    fn on_system(&mut self, msg: &str) {
        let _ = self.tx.send(ReplTuiEvent::SystemMessage(msg.to_string()));
    }

    fn on_turn_finished(&mut self, result: &Result<(), String>) {
        let _ = self.tx.send(ReplTuiEvent::TurnFinished(result.clone()));
    }

    fn on_message_completed(&mut self, msg: &ConversationMessage) {
        let _ = self.tx.send(ReplTuiEvent::MessageCompleted(msg.clone()));
    }
}

#[cfg(test)]
mod tests {
    use super::{ChannelSink, OutputSink};
    use crate::events::ReplTuiEvent;
    use render::sink::StdoutSink;
    use runtime::RuntimeObserver;
    use std::sync::mpsc::channel;

    #[test]
    fn test_channel_sink_sends_event() {
        let (tx, rx) = channel();
        let mut sink = ChannelSink::new(tx);

        sink.on_text_delta("hello");

        match rx.recv().expect("channel event") {
            ReplTuiEvent::StreamText(text) => assert_eq!(text, "hello"),
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn test_trait_object_dispatch() {
        let (tx, rx) = channel();
        let mut sink: Box<dyn OutputSink + Send> = Box::new(ChannelSink::new(tx));

        sink.on_tool_call("bash", r#"{"command":"pwd"}"#);

        match rx.recv().expect("channel event") {
            ReplTuiEvent::ToolCallStart { name, input } => {
                assert_eq!(name, "bash");
                assert_eq!(input, r#"{"command":"pwd"}"#);
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    fn forward_text(observer: &mut dyn RuntimeObserver) {
        observer.on_text_delta("observer text");
    }

    #[test]
    fn test_bridge_implements_runtime_observer() {
        let (tx, rx) = channel();
        let mut sink: Box<dyn OutputSink + Send> = Box::new(ChannelSink::new(tx));

        forward_text(&mut sink);

        match rx.recv().expect("channel event") {
            ReplTuiEvent::StreamText(text) => assert_eq!(text, "observer text"),
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn test_channel_sink_turn_finished_sends_event() {
        let (tx, rx) = channel();
        let mut sink = ChannelSink::new(tx);

        sink.on_turn_finished(&Ok(()));

        match rx.recv().expect("channel event") {
            ReplTuiEvent::TurnFinished(result) => assert_eq!(result, Ok(())),
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn test_stdout_sink_non_tty_tool_flow_from_render() {
        let mut sink = StdoutSink::with_is_tty(false);
        sink.on_tool_call("navigate", r#"{"url":"https://example.com"}"#);
        sink.on_tool_result("navigate", r#"{"ok":true}"#, false);
    }
}
