use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::Notify;

/// Control signal for managing runtime execution (cancel only).
pub struct ControlState {
    cancelled: AtomicBool,
    cancel_notify: Notify,
}

impl ControlState {
    /// Create a new `ControlState` wrapped in an `Arc`.
    #[must_use]
    pub fn new() -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self::default())
    }

    /// Request cancellation. The runtime will cancel at the next loop iteration.
    pub fn request_cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        self.cancel_notify.notify_waiters();
    }

    /// Reset the control state.
    pub fn reset(&self) {
        self.cancelled.store(false, Ordering::Release);
    }

    /// Check if the runtime is cancelled.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    /// Returns when cancellation is requested. If already cancelled, returns immediately.
    /// Use in `tokio::select!` to race against other futures.
    pub async fn cancelled(&self) {
        if self.is_cancelled() {
            return;
        }
        self.cancel_notify.notified().await;
    }
}

impl Default for ControlState {
    fn default() -> Self {
        Self {
            cancelled: AtomicBool::new(false),
            cancel_notify: Notify::new(),
        }
    }
}

// ControlState is Send + Sync because:
// - AtomicBool is Send + Sync
// - Notify is Send + Sync
// This is verified by the compiler automatically.

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn control_state_starts_not_cancelled() {
        let state = ControlState::default();
        assert!(!state.is_cancelled());
    }

    #[test]
    fn request_cancel_sets_cancelled() {
        let state = ControlState::default();
        state.request_cancel();
        assert!(state.is_cancelled());
    }

    #[test]
    fn reset_clears_cancel() {
        let state = ControlState::default();
        state.request_cancel();
        assert!(state.is_cancelled());
        state.reset();
        assert!(!state.is_cancelled());
    }

    #[tokio::test]
    async fn cancelled_returns_immediately_when_already_cancelled() {
        let state = Arc::new(ControlState::default());
        state.request_cancel();
        // Should not hang
        tokio::time::timeout(
            tokio::time::Duration::from_millis(100),
            state.cancelled(),
        )
        .await
        .expect("cancelled() should return immediately");
    }

    #[tokio::test]
    async fn cancelled_waits_for_signal() {
        use tokio::time::{sleep, timeout, Duration};

        let state = Arc::new(ControlState::default());
        let state_clone = Arc::clone(&state);

        tokio::spawn(async move {
            sleep(Duration::from_millis(50)).await;
            state_clone.request_cancel();
        });

        let result = timeout(Duration::from_secs(2), state.cancelled()).await;
        assert!(result.is_ok(), "should not timeout");
        assert!(state.is_cancelled());
    }
}
