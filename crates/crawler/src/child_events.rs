use runtime::{ControlState, RuntimeObserver};
use std::collections::HashMap;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

// ── Event types ──────────────────────────────────────────────────────────────

/// All event kinds a child agent can emit.
#[derive(Debug, Clone)]
pub enum ChildEventKind {
    StepStarted {
        step: usize,
        max_steps: usize,
    },
    TextDelta(String),
    ToolCallStart {
        name: String,
        input_summary: String,
    },
    ToolCallComplete {
        name: String,
        output_summary: String,
        is_error: bool,
    },
    PauseRequested {
        reason: String,
    },
    Resumed,
    Finished {
        success: bool,
        items_extracted: usize,
        error: Option<String>,
    },
}

/// A single event from a child agent, tagged with its identity.
#[derive(Debug, Clone)]
pub struct ChildEvent {
    pub child_id: String,
    pub sub_goal: String,
    pub event: ChildEventKind,
}

// ── ChildControlRegistry ─────────────────────────────────────────────────────

/// Thread-safe map of per-child `ControlState` instances.
/// Enables independent pause/resume/cancel per child without affecting the parent.
#[derive(Clone, Default)]
pub struct ChildControlRegistry {
    states: Arc<Mutex<HashMap<String, Arc<ControlState>>>>,
}

impl ChildControlRegistry {
    /// Register a new child and return its fresh `ControlState`.
    pub fn register(&self, child_id: &str) -> Arc<ControlState> {
        let state = Arc::new(ControlState::default());
        self.states
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(child_id.to_string(), Arc::clone(&state));
        state
    }

    /// Look up an existing child's `ControlState`.
    pub fn get(&self, child_id: &str) -> Option<Arc<ControlState>> {
        self.states
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(child_id)
            .cloned()
    }

    /// Remove a child from the registry (on completion/abort).
    pub fn remove(&self, child_id: &str) {
        self.states
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(child_id);
    }

    /// Cancel all registered children (calls `request_cancel()` on each).
    pub fn cancel_all(&self) {
        for state in self
            .states
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
        {
            state.request_cancel();
        }
    }

    /// Returns `(child_id, reason)` for all currently paused children.
    pub fn get_paused(&self) -> Vec<(String, String)> {
        self.states
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .filter(|(_, s)| s.is_paused())
            .map(|(id, s)| (id.clone(), s.pause_reason()))
            .collect()
    }
}

// ── ChildEventSender ─────────────────────────────────────────────────────────

/// Implements `RuntimeObserver` and forwards events through an mpsc channel.
/// Lives in the crawler crate (same as `ChildEvent`) to avoid cyclic dependencies.
pub struct ChildEventSender {
    child_id: String,
    sub_goal: String,
    tx: Sender<ChildEvent>,
    step: usize,
    max_steps: usize,
}

impl ChildEventSender {
    #[must_use]
    pub fn new(
        child_id: String,
        sub_goal: String,
        tx: Sender<ChildEvent>,
        max_steps: usize,
    ) -> Self {
        Self {
            child_id,
            sub_goal,
            tx,
            step: 0,
            max_steps,
        }
    }

    fn send(&self, event: ChildEventKind) {
        let _ = self.tx.send(ChildEvent {
            child_id: self.child_id.clone(),
            sub_goal: self.sub_goal.clone(),
            event,
        });
    }
}

fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let end_byte = s
            .char_indices()
            .nth(max_chars)
            .map_or(s.len(), |(idx, _)| idx);
        format!("{}…", &s[..end_byte])
    }
}

impl RuntimeObserver for ChildEventSender {
    fn on_text_delta(&mut self, text: &str) {
        self.send(ChildEventKind::TextDelta(text.to_string()));
    }

    fn on_tool_call_start(&mut self, _id: &str, name: &str, input: &str) {
        self.send(ChildEventKind::ToolCallStart {
            name: name.to_string(),
            input_summary: truncate(input, 100),
        });
    }

    fn on_tool_result(&mut self, name: &str, output: &str, is_error: bool) {
        self.send(ChildEventKind::ToolCallComplete {
            name: name.to_string(),
            output_summary: truncate(output, 100),
            is_error,
        });
    }

    fn on_pause_started(&mut self, reason: &str) {
        self.send(ChildEventKind::PauseRequested {
            reason: reason.to_string(),
        });
    }

    fn on_pause_ended(&mut self) {
        self.send(ChildEventKind::Resumed);
    }

    fn on_turn_finished(&mut self, result: &Result<(), String>) {
        self.step += 1;
        self.send(ChildEventKind::StepStarted {
            step: self.step,
            max_steps: self.max_steps,
        });
        if let Err(e) = result {
            self.send(ChildEventKind::Finished {
                success: false,
                items_extracted: 0,
                error: Some(e.clone()),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn child_control_registry_lifecycle() {
        let registry = ChildControlRegistry::default();
        let state1 = registry.register("child-1");
        assert!(!state1.is_paused());
        let state2 = registry.register("child-2");
        // Different Arc instances
        assert!(!std::ptr::eq(Arc::as_ptr(&state1), Arc::as_ptr(&state2)));
        // get() finds them
        assert!(registry.get("child-1").is_some());
        assert!(registry.get("nonexistent").is_none());
        // remove() cleans up
        registry.remove("child-1");
        assert!(registry.get("child-1").is_none());
    }

    #[test]
    fn cancel_all_propagates_to_all_children() {
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
    fn child_event_sender_delivers_text_delta() {
        let (tx, rx) = mpsc::channel::<ChildEvent>();
        let mut sender = ChildEventSender::new("child-1".into(), "test goal".into(), tx, 10);
        sender.on_text_delta("hello");
        let event = rx.try_recv().expect("event should arrive");
        assert_eq!(event.child_id, "child-1");
        match event.event {
            ChildEventKind::TextDelta(s) => assert_eq!(s, "hello"),
            other => panic!("expected TextDelta, got {other:?}"),
        }
    }

    #[test]
    fn child_event_sender_drop_does_not_panic() {
        let (tx, rx) = mpsc::channel::<ChildEvent>();
        drop(rx); // drop receiver first
        let mut sender = ChildEventSender::new("c".into(), "g".into(), tx, 5);
        sender.on_text_delta("orphan"); // must not panic
    }
}
