use std::fmt;
use std::io::{self, IsTerminal, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crossterm::cursor::{MoveDown, MoveToColumn, MoveUp};
use crossterm::style::{Attribute, Color, Print, ResetColor, SetAttribute, SetForegroundColor};
use crossterm::terminal::{Clear, ClearType};
use crossterm::{execute, queue};

use runtime::{RuntimeObserver, TokenUsage};

use crate::markdown::{MarkdownStreamState, TerminalRenderer};
use crate::tool_format::{
    format_tool_error_line, format_tool_start_line, format_tool_success_line, tool_input_summary,
};
use crate::tui::events::ReplTuiEvent;

const SPINNER_FRAMES: [char; 8] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧'];

pub trait OutputSink: Send {
    fn on_text_delta(&mut self, raw_text: &str);
    fn on_tool_call(&mut self, name: &str, input: &str);
    fn on_tool_result(&mut self, name: &str, output: &str, is_error: bool);
    fn on_system(&mut self, msg: &str);
    fn on_turn_finished(&mut self, result: &Result<(), String>);
}

pub struct StdoutSink {
    renderer: TerminalRenderer,
    markdown_stream: MarkdownStreamState,
    is_tty: bool,
    pending_tools: Arc<Mutex<Vec<(String, String)>>>,
    deferred_detail_lines: Vec<String>,
    spinner_stop: Arc<AtomicBool>,
    spinner_handle: Option<JoinHandle<()>>,
}

impl fmt::Debug for StdoutSink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StdoutSink")
            .field("is_tty", &self.is_tty)
            .finish_non_exhaustive()
    }
}

impl StdoutSink {
    #[must_use]
    pub fn new() -> Self {
        Self::with_is_tty(io::stdout().is_terminal())
    }

    fn with_is_tty(is_tty: bool) -> Self {
        Self {
            renderer: TerminalRenderer::new(),
            markdown_stream: MarkdownStreamState::default(),
            is_tty,
            pending_tools: Arc::new(Mutex::new(Vec::new())),
            deferred_detail_lines: Vec::new(),
            spinner_stop: Arc::new(AtomicBool::new(false)),
            spinner_handle: None,
        }
    }

    fn start_spinner(&mut self) {
        self.spinner_stop.store(false, Ordering::Relaxed);
        let pending_tools = Arc::clone(&self.pending_tools);
        let stop = Arc::clone(&self.spinner_stop);

        let handle = thread::spawn(move || {
            let mut frame_idx = 0usize;

            loop {
                thread::sleep(Duration::from_millis(120));
                if stop.load(Ordering::Relaxed) {
                    break;
                }

                let tools = match pending_tools.lock() {
                    Ok(guard) => guard.clone(),
                    Err(_) => break,
                };
                if tools.is_empty() {
                    break;
                }

                let frame = SPINNER_FRAMES[frame_idx % SPINNER_FRAMES.len()];
                frame_idx += 1;

                let mut stdout = io::stdout();
                let lines_up = u16::try_from(tools.len()).unwrap_or(u16::MAX);
                let _ = queue!(stdout, MoveUp(lines_up));
                for (tool_name, summary) in &tools {
                    let _ = queue!(
                        stdout,
                        MoveToColumn(0),
                        Clear(ClearType::CurrentLine),
                        SetForegroundColor(Color::Cyan),
                        SetAttribute(Attribute::Dim),
                        Print(frame),
                        ResetColor,
                        SetAttribute(Attribute::Reset),
                        Print(" "),
                        SetAttribute(Attribute::Bold),
                        Print(tool_name),
                        SetAttribute(Attribute::Reset),
                        Print(" "),
                        SetAttribute(Attribute::Dim),
                        Print(summary),
                        SetAttribute(Attribute::Reset),
                        Print("\n")
                    );
                }
                let _ = stdout.flush();
            }
        });

        self.spinner_handle = Some(handle);
    }

    fn stop_spinner(&mut self) {
        self.spinner_stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.spinner_handle.take() {
            let _ = handle.join();
        }
    }

    fn clear_pending_tools(&self) {
        if let Ok(mut pending) = self.pending_tools.lock() {
            pending.clear();
        }
    }

    fn pending_tools_snapshot(&self) -> Vec<(String, String)> {
        self.pending_tools
            .lock()
            .map(|pending| pending.clone())
            .unwrap_or_default()
    }

    fn clear_pending_block(&self) {
        if !self.is_tty {
            return;
        }

        let pending_count = self.pending_tools_snapshot().len();
        if pending_count == 0 {
            return;
        }

        let mut stdout = io::stdout();
        let lines_up = u16::try_from(pending_count).unwrap_or(u16::MAX);
        let _ = execute!(stdout, MoveUp(lines_up), MoveToColumn(0));
        for _ in 0..pending_count {
            let _ = execute!(stdout, Clear(ClearType::CurrentLine), Print("\n"));
        }
        let _ = execute!(stdout, MoveToColumn(0));
        let _ = stdout.flush();
    }

    fn render_pending_block(&self) {
        if !self.is_tty {
            return;
        }

        let tools = self.pending_tools_snapshot();
        if tools.is_empty() {
            return;
        }

        let mut stdout = io::stdout();
        for (tool_name, summary) in &tools {
            let _ = execute!(
                stdout,
                SetForegroundColor(Color::Cyan),
                Print(SPINNER_FRAMES[0]),
                ResetColor,
                Print(" "),
                SetAttribute(Attribute::Bold),
                Print(tool_name),
                SetAttribute(Attribute::Reset),
                Print(" "),
                SetAttribute(Attribute::Dim),
                Print(summary),
                SetAttribute(Attribute::Reset),
                Print("\n")
            );
        }
        let _ = stdout.flush();
    }

    fn flush_deferred_detail_lines(&mut self) {
        if self.deferred_detail_lines.is_empty() {
            return;
        }

        for detail in self.deferred_detail_lines.drain(..) {
            println!("  {detail}");
        }
    }

    fn suspend_spinner_block(&mut self) -> bool {
        let has_pending = !self.pending_tools_snapshot().is_empty();
        if self.spinner_handle.is_some() {
            self.stop_spinner();
        }
        if self.is_tty && has_pending {
            self.clear_pending_block();
        }
        has_pending
    }

    fn resume_spinner_block(&mut self, had_pending: bool) {
        if !self.is_tty || !had_pending {
            return;
        }

        self.render_pending_block();
        if self.spinner_handle.is_none() {
            self.start_spinner();
        }
    }

    fn print_tool_line(&self, line: &crate::tool_format::ToolLine, color: Option<Color>) {
        if self.is_tty {
            let mut stdout = io::stdout();
            if let Some(color) = color {
                let _ = execute!(
                    stdout,
                    SetForegroundColor(color),
                    Print(line.icon),
                    ResetColor
                );
            } else {
                let _ = execute!(stdout, Print(line.icon));
            }
            let _ = execute!(
                stdout,
                Print(" "),
                SetAttribute(Attribute::Bold),
                Print(&line.name),
                SetAttribute(Attribute::Reset),
                Print(" "),
                SetAttribute(Attribute::Dim),
                Print(&line.summary),
                SetAttribute(Attribute::Reset),
                Print("\n")
            );
            let _ = stdout.flush();
        } else {
            println!("{} {} {}", line.icon, line.name, line.summary);
        }
    }

    fn take_pending_tool(&self, name: &str) -> Option<(usize, usize, String)> {
        let mut pending = self
            .pending_tools
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let total = pending.len();
        let idx = pending
            .iter()
            .position(|(pending_name, _)| pending_name == name)?;
        let (_, summary) = pending.remove(idx);
        Some((idx, total, summary))
    }
}

impl OutputSink for StdoutSink {
    fn on_text_delta(&mut self, raw_text: &str) {
        let had_pending = self.suspend_spinner_block();
        self.flush_deferred_detail_lines();
        if let Some(rendered) = self.markdown_stream.push(&self.renderer, raw_text) {
            print!("{rendered}");
            let _ = io::stdout().flush();
        }
        self.resume_spinner_block(had_pending);
    }

    fn on_tool_call(&mut self, name: &str, input: &str) {
        if self.spinner_handle.is_some() {
            self.stop_spinner();
        }
        let line = format_tool_start_line(name, input);
        self.print_tool_line(&line, Some(Color::Cyan));

        let summary = tool_input_summary(name, input);
        if let Ok(mut pending) = self.pending_tools.lock() {
            pending.push((name.to_string(), summary));
        }

        if self.is_tty && self.spinner_handle.is_none() {
            self.start_spinner();
        }
    }

    fn on_tool_result(&mut self, name: &str, output: &str, is_error: bool) {
        self.stop_spinner();

        let pending_match = self.take_pending_tool(name);
        let summary = pending_match.as_ref().map_or_else(
            || tool_input_summary(name, "{}"),
            |(_, _, summary)| summary.clone(),
        );
        let tool_line = if is_error {
            format_tool_error_line(name, output)
        } else {
            format_tool_success_line(name, &summary, output)
        };

        if self.is_tty {
            if let Some((idx, total, _)) = pending_match.as_ref() {
                let lines_up = total - idx;
                let lines_down_after = total - idx - 1;
                let has_pending = lines_down_after > 0;
                let mut stdout = io::stdout();
                let _ = execute!(
                    stdout,
                    MoveUp(u16::try_from(lines_up).unwrap_or(u16::MAX)),
                    MoveToColumn(0),
                    Clear(ClearType::CurrentLine)
                );

                let icon_color = if is_error { Color::Red } else { Color::Green };
                let _ = execute!(
                    stdout,
                    SetForegroundColor(icon_color),
                    Print(tool_line.icon),
                    ResetColor,
                    Print(" "),
                    SetAttribute(Attribute::Bold),
                    Print(&tool_line.name),
                    SetAttribute(Attribute::Reset),
                    Print(" "),
                    SetAttribute(Attribute::Dim),
                    Print(&tool_line.summary),
                    SetAttribute(Attribute::Reset)
                );

                if lines_down_after > 0 {
                    let _ = execute!(
                        stdout,
                        MoveDown(u16::try_from(lines_down_after).unwrap_or(u16::MAX)),
                        MoveToColumn(0)
                    );
                }

                let _ = execute!(stdout, Print("\n"));
                let _ = stdout.flush();

                if has_pending {
                    self.deferred_detail_lines
                        .extend(tool_line.detail_lines.iter().cloned());
                } else {
                    for detail in &tool_line.detail_lines {
                        println!("  {detail}");
                    }
                }
            } else {
                self.print_tool_line(
                    &tool_line,
                    Some(if is_error { Color::Red } else { Color::Green }),
                );
                for detail in &tool_line.detail_lines {
                    println!("  {detail}");
                }
            }
        } else {
            println!(
                "{} {} {}",
                tool_line.icon, tool_line.name, tool_line.summary
            );
            for detail in &tool_line.detail_lines {
                println!("  {detail}");
            }
        }

        let has_pending = self
            .pending_tools
            .lock()
            .map(|pending| !pending.is_empty())
            .unwrap_or(false);
        if self.is_tty && has_pending {
            self.start_spinner();
        }
    }

    fn on_system(&mut self, msg: &str) {
        let had_pending = self.suspend_spinner_block();
        self.flush_deferred_detail_lines();
        println!("{msg}");
        self.resume_spinner_block(had_pending);
    }

    fn on_turn_finished(&mut self, result: &Result<(), String>) {
        let had_pending = self.suspend_spinner_block();
        if !had_pending {
            self.stop_spinner();
        }
        self.clear_pending_tools();
        self.flush_deferred_detail_lines();
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

impl Drop for StdoutSink {
    fn drop(&mut self) {
        self.stop_spinner();
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
    fn test_stdout_sink_non_tty_tool_flow_doesnt_panic() {
        let mut sink = StdoutSink::with_is_tty(false);

        sink.on_tool_call("navigate", r#"{"url":"https://example.com"}"#);
        sink.on_tool_result("navigate", r#"{"ok":true}"#, false);

        assert!(sink.spinner_handle.is_none());
        assert!(sink.pending_tools.lock().expect("pending lock").is_empty());
    }

    #[test]
    fn test_pending_tool_fifo_tracking() {
        let sink = StdoutSink::with_is_tty(false);
        {
            let mut pending = sink.pending_tools.lock().expect("pending lock");
            pending.push(("bash".to_string(), "first".to_string()));
            pending.push(("navigate".to_string(), "url".to_string()));
            pending.push(("bash".to_string(), "second".to_string()));
        }

        let first = sink.take_pending_tool("bash").expect("first bash");
        assert_eq!(first.0, 0);
        assert_eq!(first.1, 3);
        assert_eq!(first.2, "first");

        let second = sink.take_pending_tool("bash").expect("second bash");
        assert_eq!(second.0, 1);
        assert_eq!(second.1, 2);
        assert_eq!(second.2, "second");

        let remaining = sink.pending_tools.lock().expect("pending lock");
        assert_eq!(
            remaining.as_slice(),
            &[("navigate".to_string(), "url".to_string())]
        );
    }

    #[test]
    fn test_tool_call_starts_and_result_stops_tty_spinner() {
        let mut sink = StdoutSink::with_is_tty(true);

        sink.on_tool_call("navigate", r#"{"url":"https://example.com"}"#);
        assert!(sink.spinner_handle.is_some());

        sink.on_tool_result("navigate", r#"{"ok":true}"#, false);
        assert!(sink.spinner_handle.is_none());
        assert!(sink.pending_tools.lock().expect("pending lock").is_empty());
    }

    #[test]
    fn test_defer_detail_lines_while_pending_tools_remain() {
        let mut sink = StdoutSink::with_is_tty(true);
        {
            let mut pending = sink.pending_tools.lock().expect("pending lock");
            pending.push(("bash".to_string(), "first".to_string()));
            pending.push(("navigate".to_string(), "url".to_string()));
        }

        sink.on_tool_result("bash", r#"{"stdout":"line1\nline2","stderr":""}"#, false);

        assert_eq!(
            sink.deferred_detail_lines,
            vec!["line1".to_string(), "line2".to_string()]
        );
        assert_eq!(
            sink.pending_tools.lock().expect("pending lock").as_slice(),
            &[("navigate".to_string(), "url".to_string())]
        );

        sink.stop_spinner();
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
