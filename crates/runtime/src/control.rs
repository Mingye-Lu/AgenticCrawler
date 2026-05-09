use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use tokio::sync::Notify;

/// Control signal states for the runtime.
pub const CONTINUE: u8 = 0;
pub const PAUSE: u8 = 1;
pub const CANCEL: u8 = 2;

/// Tri-state control signal for managing runtime execution.
///
/// Replaces the simple `AtomicBool` cancel flag with a more expressive state machine
/// that supports pause, cancel, and resume operations.
pub struct ControlState {
    signal: AtomicU8,
    resume_notify: Notify,
}

impl ControlState {
    /// Create a new `ControlState` wrapped in an `Arc`.
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            signal: AtomicU8::new(CONTINUE),
            resume_notify: Notify::new(),
        })
    }

    /// Request a pause. The runtime will pause at the next loop iteration.
    pub fn request_pause(&self) {
        self.signal.store(PAUSE, Ordering::Release);
    }

    /// Request cancellation. The runtime will cancel at the next loop iteration.
    pub fn request_cancel(&self) {
        self.signal.store(CANCEL, Ordering::Release);
    }

    /// Resume from a paused state and clear the pause signal.
    pub fn resume(&self) {
        self.signal.store(CONTINUE, Ordering::Release);
        self.resume_notify.notify_waiters();
    }

    /// Reset the control state to `CONTINUE`.
    pub fn reset(&self) {
        self.signal.store(CONTINUE, Ordering::Release);
    }

    /// Check if the runtime is paused.
    #[must_use]
    pub fn is_paused(&self) -> bool {
        self.signal.load(Ordering::Acquire) == PAUSE
    }

    /// Check if the runtime is cancelled.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.signal.load(Ordering::Acquire) == CANCEL
    }

    /// Wait for a resume signal. Returns when `resume()` is called.
    pub async fn wait_for_resume(&self) {
        self.resume_notify.notified().await;
    }
}

impl Default for ControlState {
    fn default() -> Self {
        Self {
            signal: AtomicU8::new(CONTINUE),
            resume_notify: Notify::new(),
        }
    }
}

// ControlState is Send + Sync because:
// - AtomicU8 is Send + Sync
// - Notify is Send + Sync
// This is verified by the compiler automatically.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_state_starts_in_continue() {
        let state = ControlState::default();
        assert!(!state.is_paused());
        assert!(!state.is_cancelled());
    }

    #[test]
    fn request_pause_sets_paused() {
        let state = ControlState::default();
        state.request_pause();
        assert!(state.is_paused());
        assert!(!state.is_cancelled());
    }

    #[test]
    fn request_cancel_sets_cancelled() {
        let state = ControlState::default();
        state.request_cancel();
        assert!(state.is_cancelled());
        assert!(!state.is_paused());
    }

    #[test]
    fn resume_clears_pause() {
        let state = ControlState::default();
        state.request_pause();
        assert!(state.is_paused());
        state.resume();
        assert!(!state.is_paused());
        assert!(!state.is_cancelled());
    }

    #[test]
    fn reset_clears_cancel() {
        let state = ControlState::default();
        state.request_cancel();
        assert!(state.is_cancelled());
        state.reset();
        assert!(!state.is_cancelled());
        assert!(!state.is_paused());
    }

    #[tokio::test]
    async fn wait_for_resume_returns_after_resume() {
        let state = Arc::new(ControlState::default());
        let state_clone = Arc::clone(&state);

        let wait_task = tokio::spawn(async move {
            state_clone.wait_for_resume().await;
        });

        // Give the wait task time to start waiting
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        // Resume should wake up the waiter
        state.resume();

        // The wait task should complete without timeout
        tokio::time::timeout(
            tokio::time::Duration::from_secs(1),
            wait_task,
        )
        .await
        .expect("wait_for_resume should return after resume()")
        .expect("wait task should not panic");
    }
}
