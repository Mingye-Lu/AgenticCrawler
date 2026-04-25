use std::io::{self, Write};
use std::sync::mpsc;

use runtime::{RuntimeObserver, TokenUsage};

use crate::markdown::{MarkdownStreamState, TerminalRenderer};
use crate::tui::events::ReplTuiEvent;
use crate::tool_format::{format_tool_call_start, format_tool_result};

pub trait OutputSink: Send {
    fn on_text_delta(&mut self, raw_text: &str);
    fn on_tool_call(&mut self, name: &str, input: &str);
    fn on_tool_result(&mut self, name: &str, output: &str, is_error: bool);
    fn on_system(&mut self, msg: &str);
    fn on_turn_finished(&mut self, result: &Result<(), String>);
}

#[derive(Debug)]
pub struct StdoutSink {
    renderer: TerminalRenderer,
    markdown_stream: MarkdownStreamState,
}

impl StdoutSink {
    #[must_use]
    pub fn new() -> Self {
        Self {
            renderer: TerminalRenderer::new(),
            markdown_stream: MarkdownStreamState::default(),
        }
    }
}

impl OutputSink for StdoutSink {
    fn on_text_delta(&mut self, raw_text: &str) {
        if let Some(rendered) = self.markdown_stream.push(&self.renderer, raw_text) {
            print!("{rendered}");
            let _ = io::stdout().flush();
        }
    }

    fn on_tool_call(&mut self, name: &str, input: &str) {
        println!("{}", format_tool_call_start(name, input));
    }

    fn on_tool_result(&mut self, name: &str, output: &str, is_error: bool) {
        println!("{}", format_tool_result(name, output, is_error));
    }

    fn on_system(&mut self, msg: &str) {
        println!("{msg}");
    }

    fn on_turn_finished(&mut self, result: &Result<(), String>) {
        if let Some(rendered) = self.markdown_stream.flush(&self.renderer) {
            print!("{rendered}");
            let _ = io::stdout().flush();
        }
        match result {
            Ok(()) => println!("\n✔ Turn complete"),
            Err(error) => eprintln!("\n✘ Turn failed: {error}"),
        }
    }
}

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
        let _ = self.tx.send(ReplTuiEvent::StreamAnsi(raw_text.to_string()));
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
}

impl RuntimeObserver for Box<dyn OutputSink + Send + '_> {
    fn on_text_delta(&mut self, text: &str) {
        (**self).on_text_delta(text);
    }

    fn on_tool_call_start(&mut self, _id: &str, name: &str, input: &str) {
        (**self).on_tool_call(name, input);
    }

    fn on_tool_result(&mut self, name: &str, output: &str, is_error: bool) {
        (**self).on_tool_result(name, output, is_error);
    }

    fn on_system_message(&mut self, msg: &str) {
        (**self).on_system(msg);
    }

    fn on_turn_finished(&mut self, result: &Result<(), String>) {
        (**self).on_turn_finished(result);
    }

    fn on_usage(&mut self, _usage: &TokenUsage) {}
}

#[cfg(test)]
mod tests {
    use super::{ChannelSink, OutputSink, StdoutSink};
    use crate::tui::events::ReplTuiEvent;
    use runtime::RuntimeObserver;
    use std::sync::mpsc::channel;

    #[test]
    fn test_stdout_sink_on_text_delta_doesnt_panic() {
        let mut sink = StdoutSink::new();
        sink.on_text_delta("hello");
    }

    #[test]
    fn test_channel_sink_sends_event() {
        let (tx, rx) = channel();
        let mut sink = ChannelSink::new(tx);

        sink.on_text_delta("hello");

        match rx.recv().expect("channel event") {
            ReplTuiEvent::StreamAnsi(text) => assert_eq!(text, "hello"),
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
            ReplTuiEvent::StreamAnsi(text) => assert_eq!(text, "observer text"),
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
}
