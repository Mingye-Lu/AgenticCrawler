use std::sync::mpsc;
use std::time::Duration;

use crawler::{ChildControlRegistry, ChildEvent, ChildEventKind, ChildEventSender};
use runtime::RuntimeObserver;

#[allow(dead_code)]
mod markdown {
    #[derive(Clone)]
    pub struct PredictiveMarkdownBuffer;

    impl PredictiveMarkdownBuffer {
        pub fn new() -> Self {
            Self
        }

        #[allow(clippy::unused_self)]
        pub fn feed_char(&mut self, _c: char, _out: &mut String) {}

        #[allow(clippy::unused_self)]
        pub fn flush(&mut self, _out: &mut String) {}
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
    pub fn ansi_to_lines(_ansi: &str) -> Vec<ratatui::text::Line<'static>> {
        Vec::new()
    }
}

#[allow(dead_code)]
#[path = "../src/tui/child_tabs.rs"]
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
        &ChildEventKind::PauseRequested {
            reason: "captcha".to_string(),
        },
    );
    assert!(matches!(
        panel.tabs[0].status,
        ChildTabStatus::Paused { ref reason } if reason == "captcha"
    ));

    panel.apply_event("child-1", "collect results", &ChildEventKind::Resumed);
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
fn cancel_during_pause_unblocks_child() {
    let registry = ChildControlRegistry::default();
    let child_state = registry.register("child-1");
    child_state.request_pause_with_reason("waiting for human");
    assert!(child_state.is_paused());

    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    runtime.block_on(async {
        let state_for_wait = child_state.clone();
        let wait_task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    () = state_for_wait.wait_for_resume() => break,
                    () = tokio::time::sleep(Duration::from_millis(20)) => {
                        if state_for_wait.is_cancelled() || !state_for_wait.is_paused() {
                            break;
                        }
                    }
                }
            }
        });

        tokio::time::sleep(Duration::from_millis(50)).await;
        registry.cancel_all();

        tokio::time::timeout(Duration::from_secs(1), wait_task)
            .await
            .expect("cancel should unblock paused child")
            .expect("wait task should not panic");
    });

    assert!(child_state.is_cancelled());
}

#[test]
fn two_children_pause_resume_independently() {
    let mut panel = ChildTabPanel::default();

    panel.apply_event(
        "c1",
        "goal-1",
        &ChildEventKind::PauseRequested {
            reason: "captcha1".to_string(),
        },
    );
    panel.apply_event(
        "c2",
        "goal-2",
        &ChildEventKind::PauseRequested {
            reason: "captcha2".to_string(),
        },
    );

    assert!(matches!(
        panel.tabs[0].status,
        ChildTabStatus::Paused { .. }
    ));
    assert!(matches!(
        panel.tabs[1].status,
        ChildTabStatus::Paused { .. }
    ));

    panel.apply_event("c1", "goal-1", &ChildEventKind::Resumed);
    assert_eq!(panel.tabs[0].status, ChildTabStatus::Running);
    assert!(matches!(
        panel.tabs[1].status,
        ChildTabStatus::Paused { .. }
    ));

    panel.apply_event("c2", "goal-2", &ChildEventKind::Resumed);
    assert_eq!(panel.tabs[1].status, ChildTabStatus::Running);
}

#[test]
fn event_channel_delivers_all_event_types() {
    let (tx, rx) = mpsc::channel::<ChildEvent>();
    let mut sender = ChildEventSender::new("c1".into(), "goal".into(), tx, 15);

    sender.on_text_delta("delta text");
    sender.on_tool_call_start("id1", "navigate", r#"{"url":"x"}"#);
    sender.on_tool_result("navigate", "result", false);
    sender.on_pause_started("need input");
    sender.on_pause_ended();
    sender.on_turn_finished(&Err("boom".to_string()));

    let events = drain(&rx);
    assert_eq!(events.len(), 7);
    assert!(matches!(events[0], ChildEventKind::TextDelta(_)));
    assert!(matches!(events[1], ChildEventKind::ToolCallStart { .. }));
    assert!(matches!(events[2], ChildEventKind::ToolCallComplete { .. }));
    assert!(matches!(events[3], ChildEventKind::PauseRequested { .. }));
    assert!(matches!(events[4], ChildEventKind::Resumed));
    assert!(matches!(events[5], ChildEventKind::StepStarted { .. }));
    assert!(matches!(events[6], ChildEventKind::Finished { .. }));
}

#[test]
fn cancel_all_does_not_affect_already_done_children() {
    let registry = ChildControlRegistry::default();
    let state = registry.register("c1");
    registry.remove("c1");

    registry.cancel_all();

    assert!(!state.is_cancelled());
}
