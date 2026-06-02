use std::sync::mpsc;

use agent::{ChildControlRegistry, ChildEvent, ChildEventKind, ChildEventSender};
use runtime::RuntimeObserver;

#[allow(dead_code)]
mod markdown {
    use ratatui::text::Line;

    pub fn render_lines(input: &str) -> Vec<Line<'static>> {
        if input.is_empty() {
            Vec::new()
        } else {
            vec![Line::from(input.to_string())]
        }
    }

    pub fn drain_safe_boundary(buffer: &mut String) -> Option<Vec<Line<'static>>> {
        let idx = buffer.find('\n')?;
        let chunk: String = buffer.drain(..=idx).collect();
        Some(render_lines(&chunk))
    }
}

#[allow(dead_code)]
mod repl_app {
    #[derive(Clone, Debug)]
    pub enum ToolCallStatus {
        Running,
        Interrupted,
        Success { output: String },
        Error(String),
    }

    #[derive(Clone)]
    pub enum TranscriptEntry {
        System(String),
        Status(String),
        User(String),
        Parent(String),
        Stream(ratatui::text::Line<'static>),
        SystemCard {
            title: String,
            rows: Vec<(String, String)>,
        },
        ToolCall {
            name: String,
            input_summary: String,
            status: ToolCallStatus,
        },
    }
}

#[allow(dead_code)]
mod repl_render {
    pub fn ansi_to_lines(ansi: &str) -> Vec<ratatui::text::Line<'static>> {
        if ansi.is_empty() {
            Vec::new()
        } else {
            vec![ratatui::text::Line::from(ansi.to_string())]
        }
    }
}

#[allow(dead_code)]
#[path = "../../tui/src/child_tabs.rs"]
mod child_tabs;

use child_tabs::{ChildTabPanel, ChildTabStatus};

fn drain(rx: &mpsc::Receiver<ChildEvent>) -> Vec<ChildEventKind> {
    let mut events = Vec::new();
    while let Ok(event) = rx.try_recv() {
        events.push(event.event);
    }
    events
}

#[test]
fn tab_state_transitions_via_events() {
    let mut panel = ChildTabPanel::default();

    panel.apply_event(
        "child-1",
        "collect results",
        &ChildEventKind::StepStarted {
            step: 1,
            max_steps: 4,
        },
    );
    assert_eq!(panel.tabs[0].status, ChildTabStatus::Running);

    panel.apply_event(
        "child-1",
        "collect results",
        &ChildEventKind::Finished {
            success: true,
            items_extracted: 2,
            error: None,
        },
    );
    assert_eq!(panel.tabs[0].status, ChildTabStatus::Done);
    assert_eq!(panel.tabs[0].items_extracted, 2);
}

#[test]
fn event_channel_delivers_all_event_types() {
    let (tx, rx) = mpsc::channel::<ChildEvent>();
    let mut sender = ChildEventSender::new("c1".into(), "goal".into(), tx, 15);

    sender.on_text_delta("delta text");
    sender.on_tool_call_start("id1", "navigate", r#"{"url":"x"}"#);
    sender.on_tool_result("navigate", "result", false);
    sender.on_turn_finished(&Err("boom".to_string()));

    let events = drain(&rx);
    assert_eq!(events.len(), 5);
    assert!(matches!(events[0], ChildEventKind::TextDelta(_)));
    assert!(matches!(events[1], ChildEventKind::ToolCallStart { .. }));
    assert!(matches!(events[2], ChildEventKind::ToolCallComplete { .. }));
    assert!(matches!(events[3], ChildEventKind::StepStarted { .. }));
    assert!(matches!(events[4], ChildEventKind::Finished { .. }));
}

#[test]
fn cancel_all_does_not_affect_already_done_children() {
    let registry = ChildControlRegistry::default();
    let state = registry.register("c1");
    registry.remove("c1");

    registry.cancel_all();

    assert!(!state.is_cancelled());
}
