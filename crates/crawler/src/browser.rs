use tokio::sync::MutexGuard;

use crate::{PlaywrightBridge, PlaywrightBridgeError, SharedBridge};

#[derive(Debug, Clone)]
pub struct BrowserContext {
    bridge: SharedBridge,
    page_index: usize,
}

impl BrowserContext {
    #[must_use]
    pub fn new(bridge: SharedBridge) -> Self {
        Self::new_shared(bridge, 0)
    }

    #[must_use]
    pub fn new_shared(bridge: SharedBridge, page_index: usize) -> Self {
        Self { bridge, page_index }
    }

    #[must_use]
    pub fn bridge(&self) -> &SharedBridge {
        &self.bridge
    }

    pub async fn acquire_bridge(
        &self,
    ) -> Result<MutexGuard<'_, PlaywrightBridge>, PlaywrightBridgeError> {
        let page_idx = i64::try_from(self.page_index).unwrap_or(0);
        let mut guard = self.bridge.lock().await;
        guard.switch_tab(page_idx).await?;
        Ok(guard)
    }

    #[must_use]
    pub fn page_index(&self) -> usize {
        self.page_index
    }

    pub fn set_page_index(&mut self, page_index: usize) {
        self.page_index = page_index;
    }
}
