use crate::state::CrawlState;
use crate::BrowserContext;

/// Atomically advances the global seq counter and pushes the new value to the
/// bridge so the just-completed action's observations are tagged for temporal
/// filtering. Returns the seq assigned to the action (the pre-advance value).
pub async fn increment_seq(state: &CrawlState, browser: &mut BrowserContext) -> u64 {
    let new_seq = state.seq_counter.next();
    let action_seq = new_seq.saturating_sub(1);
    {
        let mut bridge = browser.bridge().lock().await;
        let _ = bridge.set_seq(new_seq).await;
    }
    action_seq
}
