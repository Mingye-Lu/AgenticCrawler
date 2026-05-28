use std::sync::mpsc;

use acrawl_core::RuntimeObserver;
use agent::{ChildControlRegistry, ChildEvent, ChildEventKind, ChildEventSender};

#[test]
fn sender_delivers_text_delta() {
    let (tx, rx) = mpsc::channel::<ChildEvent>();
    let mut sender = ChildEventSender::new("c1".into(), "goal".into(), tx, 10);
    sender.on_text_delta("hello world");
    let event = rx.try_recv().expect("event should arrive");
    assert_eq!(event.child_id, "c1");
    match event.event {
        ChildEventKind::TextDelta(text) => assert_eq!(text, "hello world"),
        other => panic!("expected TextDelta, got {other:?}"),
    }
}

#[test]
fn sender_delivers_tool_call_start() {
    let (tx, rx) = mpsc::channel::<ChildEvent>();
    let mut sender = ChildEventSender::new("c1".into(), "goal".into(), tx, 10);
    sender.on_tool_call_start("id1", "navigate", r#"{"url":"https://example.com"}"#);
    let event = rx.try_recv().expect("event should arrive");
    match event.event {
        ChildEventKind::ToolCallStart { name, .. } => assert_eq!(name, "navigate"),
        other => panic!("expected ToolCallStart, got {other:?}"),
    }
}

#[test]
fn sender_delivers_tool_result() {
    let (tx, rx) = mpsc::channel::<ChildEvent>();
    let mut sender = ChildEventSender::new("c1".into(), "goal".into(), tx, 10);
    sender.on_tool_result("navigate", "page content here", false);
    let event = rx.try_recv().expect("event should arrive");
    match event.event {
        ChildEventKind::ToolCallComplete { name, is_error, .. } => {
            assert_eq!(name, "navigate");
            assert!(!is_error);
        }
        other => panic!("expected ToolCallComplete, got {other:?}"),
    }
}

#[test]
fn sender_delivers_pause_and_resume() {
    let (tx, rx) = mpsc::channel::<ChildEvent>();
    let mut sender = ChildEventSender::new("c1".into(), "goal".into(), tx, 10);
    sender.on_pause_started("captcha detected");
    let pause_event = rx.try_recv().expect("pause event should arrive");
    match pause_event.event {
        ChildEventKind::PauseRequested { reason } => assert_eq!(reason, "captcha detected"),
        other => panic!("expected PauseRequested, got {other:?}"),
    }

    sender.on_pause_ended();
    let resume_event = rx.try_recv().expect("resume event should arrive");
    assert!(matches!(resume_event.event, ChildEventKind::Resumed));
}

#[test]
fn sender_emits_step_started_and_finished_error_on_failed_turn() {
    let (tx, rx) = mpsc::channel::<ChildEvent>();
    let mut sender = ChildEventSender::new("c1".into(), "goal".into(), tx, 10);
    sender.on_turn_finished(&Err("boom".to_string()));

    let step_event = rx.try_recv().expect("step event should arrive");
    assert!(matches!(
        step_event.event,
        ChildEventKind::StepStarted {
            step: 1,
            max_steps: 10
        }
    ));

    let finished_event = rx.try_recv().expect("finished event should arrive");
    assert!(matches!(
        finished_event.event,
        ChildEventKind::Finished {
            success: false,
            items_extracted: 0,
            error: Some(ref error)
        } if error == "boom"
    ));
}

#[test]
fn sender_drop_does_not_panic() {
    let (tx, rx) = mpsc::channel::<ChildEvent>();
    drop(rx);
    let mut sender = ChildEventSender::new("c".into(), "g".into(), tx, 5);
    sender.on_text_delta("orphan");
}

#[test]
fn sender_tags_all_events_with_child_id() {
    let (tx, rx) = mpsc::channel::<ChildEvent>();
    let mut sender = ChildEventSender::new("my-child".into(), "my-goal".into(), tx, 10);
    sender.on_text_delta("a");
    sender.on_tool_call_start("id", "tool", "{}");

    while let Ok(event) = rx.try_recv() {
        assert_eq!(event.child_id, "my-child");
        assert_eq!(event.sub_goal, "my-goal");
    }
}

#[test]
fn registry_register_get_remove() {
    let registry = ChildControlRegistry::default();
    let state = registry.register("c1");
    assert!(!state.is_paused());
    assert!(registry.get("c1").is_some());
    assert!(registry.get("nonexistent").is_none());
    registry.remove("c1");
    assert!(registry.get("c1").is_none());
}

#[test]
fn registry_cancel_all_cancels_all_children() {
    let registry = ChildControlRegistry::default();
    let s1 = registry.register("c1");
    let s2 = registry.register("c2");
    let s3 = registry.register("c3");
    registry.cancel_all();
    assert!(s1.is_cancelled());
    assert!(s2.is_cancelled());
    assert!(s3.is_cancelled());
}

#[test]
fn registry_get_paused_returns_only_paused() {
    let registry = ChildControlRegistry::default();
    let s1 = registry.register("c1");
    let _s2 = registry.register("c2");
    s1.request_pause_with_reason("needs captcha");
    let paused = registry.get_paused();
    assert_eq!(paused.len(), 1);
    assert_eq!(paused[0].0, "c1");
    assert_eq!(paused[0].1, "needs captcha");
}
