use tokio::sync::MutexGuard;

use crate::{PlaywrightBridge, SharedBridge};

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

    pub async fn acquire_bridge(&self) -> MutexGuard<'_, PlaywrightBridge> {
        self.bridge.lock().await
    }

    #[must_use]
    pub fn page_index(&self) -> usize {
        self.page_index
    }
}
