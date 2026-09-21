use crate::state::CrawlState;
use crate::BrowserContext;

/// Atomically advances the global seq counter and pushes the new value to the
/// bridge so the just-completed action's observations are tagged for temporal
/// filtering. Returns the seq assigned to the action (the pre-advance value).
///
/// This is called by nearly every mutating tool, so a failed `set_seq` isn't
/// treated as fatal to the calling tool (its own result is more valuable to
/// the caller than a seq push failing) — but silently discarding the error
/// would leave the bridge's temporal tagging desynced from `CrawlState`'s
/// counter, corrupting `since`/`until` windowing used by `network_activity`,
/// `websocket_activity`, and `page_logs`. Surface it to stderr instead.
pub async fn increment_seq(state: &CrawlState, browser: &mut BrowserContext) -> u64 {
    let new_seq = state.seq_counter.next();
    let action_seq = new_seq.saturating_sub(1);
    {
        let mut bridge = browser.bridge().lock().await;
        if let Err(error) = bridge.set_seq(new_seq).await {
            eprintln!(
                "Warning: failed to set bridge seq to {new_seq}: {error} \
                 (since/until windowing for network_activity/websocket_activity/page_logs may desync)"
            );
        }
    }
    action_seq
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn increment_seq_still_returns_action_seq_when_bridge_set_seq_fails() {
        let mut browser = crate::tools::test_support::browser_with_failing_set_seq();
        let state = CrawlState::default();

        // A failing bridge.set_seq() must not panic or block the caller —
        // the seq counter still advances and the assigned action_seq is
        // still returned, even though the bridge-side push failed.
        let first = increment_seq(&state, &mut browser).await;
        let second = increment_seq(&state, &mut browser).await;

        assert_eq!(first, 0);
        assert_eq!(second, 1);
    }
}
