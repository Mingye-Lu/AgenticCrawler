use crate::state::CrawlState;
use crate::BrowserContext;

/// Increments the global seq counter and best-effort notifies the bridge.
/// Returns the seq value that tagged the just-completed action's observations.
pub async fn increment_seq(state: &CrawlState, browser: &mut BrowserContext) -> u64 {
    let action_seq = state.seq_counter.current();
    let new_seq = state.seq_counter.next();
    if let Ok(mut bridge) = browser.bridge().try_lock() {
        let _ = bridge.set_seq(new_seq).await;
    }
    action_seq
}
