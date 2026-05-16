use runtime::{ControlState, RuntimeObserver};
use std::collections::HashMap;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::Instant;

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

// ── ChildSnapshotRegistry ────────────────────────────────────────────────────

const LAST_TEXT_MAX_CHARS: usize = 200;
const LAST_TOOL_INPUT_MAX_CHARS: usize = 100;

/// Lifecycle state of a child agent as seen by the parent. Updated whenever
/// the corresponding observer hook fires, plus on cancel/finish in the wait
/// path. Reported by the `subagent_status` tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChildLifecycle {
    /// Registered but no events yet (or no LLM turn yet).
    Created,
    /// At least one observer event seen and the child hasn't ended.
    Running,
    /// The child requested a pause (waiting for human / cancel).
    Paused,
    /// Child returned successfully.
    Completed,
    /// Child returned with an error (panic, hard runtime error).
    Failed,
    /// Cancelled by parent via `cancel_subagent` or `Drop`.
    Cancelled,
}

impl ChildLifecycle {
    /// Wire format used by the LLM-facing `subagent_status` payload.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Running => "running",
            Self::Paused => "paused",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

/// A point-in-time picture of one child agent's progress. Populated by
/// `ChildEventSender` from inside the child's runtime; read by
/// `subagent_status` from the parent.
#[derive(Debug, Clone)]
pub struct ChildSnapshot {
    pub child_id: String,
    pub sub_goal: String,
    pub state: ChildLifecycle,
    pub step: usize,
    pub max_steps: usize,
    pub last_tool: Option<String>,
    pub last_text: Option<String>,
    pub items_extracted: usize,
    pub error: Option<String>,
    pub last_event_at: Instant,
}

impl ChildSnapshot {
    fn new(child_id: String, sub_goal: String, max_steps: usize) -> Self {
        Self {
            child_id,
            sub_goal,
            state: ChildLifecycle::Created,
            step: 0,
            max_steps,
            last_tool: None,
            last_text: None,
            items_extracted: 0,
            error: None,
            last_event_at: Instant::now(),
        }
    }
}

/// Concurrent map of `ChildSnapshot`s keyed by child id. Cloning is cheap
/// (Arc); the same registry instance is shared between parent and child
/// agents.
#[derive(Clone, Default)]
pub struct ChildSnapshotRegistry {
    inner: Arc<Mutex<HashMap<String, ChildSnapshot>>>,
}

impl ChildSnapshotRegistry {
    /// Insert a fresh snapshot for a newly-spawned child. Overwrites any
    /// stale entry from a previous lifecycle.
    pub fn register(&self, child_id: &str, sub_goal: &str, max_steps: usize) {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(
                child_id.to_string(),
                ChildSnapshot::new(child_id.to_string(), sub_goal.to_string(), max_steps),
            );
    }

    /// Apply a mutation under the lock. The closure receives `&mut
    /// ChildSnapshot`; if the entry is missing the closure is not invoked.
    pub fn update_with<F: FnOnce(&mut ChildSnapshot)>(&self, child_id: &str, f: F) {
        let mut guard = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(snapshot) = guard.get_mut(child_id) {
            snapshot.last_event_at = Instant::now();
            f(snapshot);
        }
    }

    /// Convenience: set the lifecycle state. Bumps `last_event_at`.
    pub fn set_state(&self, child_id: &str, state: ChildLifecycle) {
        self.update_with(child_id, |snapshot| snapshot.state = state);
    }

    /// Convenience used by the cancel path: mark a child as Cancelled with
    /// an error message.
    pub fn mark_cancelled(&self, child_id: &str, reason: &str) {
        self.update_with(child_id, |snapshot| {
            snapshot.state = ChildLifecycle::Cancelled;
            snapshot.error = Some(reason.to_string());
        });
    }

    #[must_use]
    pub fn get(&self, child_id: &str) -> Option<ChildSnapshot> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(child_id)
            .cloned()
    }

    /// Snapshot every entry under the lock and return as a Vec.
    #[must_use]
    pub fn list(&self) -> Vec<ChildSnapshot> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .cloned()
            .collect()
    }

    pub fn remove(&self, child_id: &str) {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(child_id);
    }
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
///
/// Also mirrors every observer callback into an optional
/// [`ChildSnapshotRegistry`] so the parent can poll a child's state without
/// joining or cancelling it (see the `subagent_status` tool).
pub struct ChildEventSender {
    child_id: String,
    sub_goal: String,
    tx: Sender<ChildEvent>,
    step: usize,
    max_steps: usize,
    snapshots: Option<ChildSnapshotRegistry>,
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
            snapshots: None,
        }
    }

    /// Attach a snapshot registry. The sender will mirror its observer
    /// callbacks into the registry entry for `child_id`.
    #[must_use]
    pub fn with_snapshots(mut self, snapshots: ChildSnapshotRegistry) -> Self {
        self.snapshots = Some(snapshots);
        self
    }

    fn send(&self, event: ChildEventKind) {
        let _ = self.tx.send(ChildEvent {
            child_id: self.child_id.clone(),
            sub_goal: self.sub_goal.clone(),
            event,
        });
    }

    fn update_snapshot<F: FnOnce(&mut ChildSnapshot)>(&self, f: F) {
        if let Some(snapshots) = &self.snapshots {
            snapshots.update_with(&self.child_id, f);
        }
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
        let truncated = truncate(text, LAST_TEXT_MAX_CHARS);
        self.update_snapshot(|snapshot| {
            snapshot.last_text = Some(truncated);
            if snapshot.state == ChildLifecycle::Created {
                snapshot.state = ChildLifecycle::Running;
            }
        });
    }

    fn on_tool_call_start(&mut self, _id: &str, name: &str, input: &str) {
        self.send(ChildEventKind::ToolCallStart {
            name: name.to_string(),
            input_summary: truncate(input, LAST_TOOL_INPUT_MAX_CHARS),
        });
        let tool_name = name.to_string();
        self.update_snapshot(|snapshot| {
            snapshot.last_tool = Some(tool_name);
            if snapshot.state == ChildLifecycle::Created {
                snapshot.state = ChildLifecycle::Running;
            }
        });
    }

    fn on_tool_result(&mut self, name: &str, output: &str, is_error: bool) {
        self.send(ChildEventKind::ToolCallComplete {
            name: name.to_string(),
            output_summary: truncate(output, LAST_TOOL_INPUT_MAX_CHARS),
            is_error,
        });
        // Tool result is also a heartbeat — bump the timestamp.
        self.update_snapshot(|_| {});
    }

    fn on_pause_started(&mut self, reason: &str) {
        self.send(ChildEventKind::PauseRequested {
            reason: reason.to_string(),
        });
        self.update_snapshot(|snapshot| snapshot.state = ChildLifecycle::Paused);
    }

    fn on_pause_ended(&mut self) {
        self.send(ChildEventKind::Resumed);
        self.update_snapshot(|snapshot| snapshot.state = ChildLifecycle::Running);
    }

    fn on_turn_finished(&mut self, result: &Result<(), String>) {
        self.step += 1;
        let step_now = self.step;
        let max_steps = self.max_steps;
        self.send(ChildEventKind::StepStarted {
            step: step_now,
            max_steps,
        });
        self.update_snapshot(|snapshot| {
            snapshot.step = step_now;
            if snapshot.state == ChildLifecycle::Created {
                snapshot.state = ChildLifecycle::Running;
            }
        });
        if let Err(e) = result {
            self.send(ChildEventKind::Finished {
                success: false,
                items_extracted: 0,
                error: Some(e.clone()),
            });
            let error_text = e.clone();
            self.update_snapshot(|snapshot| {
                snapshot.state = ChildLifecycle::Failed;
                snapshot.error = Some(error_text);
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

    #[test]
    fn snapshot_registry_lifecycle() {
        let registry = ChildSnapshotRegistry::default();
        registry.register("c1", "goal", 10);
        let snapshot = registry.get("c1").expect("snapshot should exist");
        assert_eq!(snapshot.sub_goal, "goal");
        assert_eq!(snapshot.state, ChildLifecycle::Created);
        assert_eq!(snapshot.max_steps, 10);

        registry.set_state("c1", ChildLifecycle::Running);
        assert_eq!(registry.get("c1").unwrap().state, ChildLifecycle::Running);

        registry.remove("c1");
        assert!(registry.get("c1").is_none());
    }

    #[test]
    fn snapshot_updates_on_observer_callbacks() {
        let (tx, _rx) = mpsc::channel::<ChildEvent>();
        let snapshots = ChildSnapshotRegistry::default();
        snapshots.register("c1", "goal", 5);
        let mut sender = ChildEventSender::new("c1".into(), "goal".into(), tx, 5)
            .with_snapshots(snapshots.clone());

        sender.on_tool_call_start("call-1", "navigate", "{}");
        let s = snapshots.get("c1").unwrap();
        assert_eq!(s.state, ChildLifecycle::Running);
        assert_eq!(s.last_tool.as_deref(), Some("navigate"));

        sender.on_text_delta("hello world");
        let s = snapshots.get("c1").unwrap();
        assert_eq!(s.last_text.as_deref(), Some("hello world"));

        sender.on_turn_finished(&Err("boom".to_string()));
        let s = snapshots.get("c1").unwrap();
        assert_eq!(s.state, ChildLifecycle::Failed);
        assert_eq!(s.error.as_deref(), Some("boom"));
        assert_eq!(s.step, 1);
    }

    #[test]
    fn snapshot_last_text_truncates_to_200_chars() {
        let (tx, _rx) = mpsc::channel::<ChildEvent>();
        let snapshots = ChildSnapshotRegistry::default();
        snapshots.register("c1", "goal", 5);
        let mut sender = ChildEventSender::new("c1".into(), "goal".into(), tx, 5)
            .with_snapshots(snapshots.clone());
        let huge = "x".repeat(500);
        sender.on_text_delta(&huge);
        let last_text = snapshots.get("c1").unwrap().last_text.unwrap();
        // 200 chars + 1 ellipsis = 201 chars in last_text.
        assert_eq!(last_text.chars().count(), 201);
    }

    #[test]
    fn snapshot_pause_resume_flips_lifecycle() {
        let (tx, _rx) = mpsc::channel::<ChildEvent>();
        let snapshots = ChildSnapshotRegistry::default();
        snapshots.register("c1", "goal", 5);
        let mut sender = ChildEventSender::new("c1".into(), "goal".into(), tx, 5)
            .with_snapshots(snapshots.clone());

        sender.on_pause_started("captcha");
        assert_eq!(snapshots.get("c1").unwrap().state, ChildLifecycle::Paused);
        sender.on_pause_ended();
        assert_eq!(snapshots.get("c1").unwrap().state, ChildLifecycle::Running);
    }
}
