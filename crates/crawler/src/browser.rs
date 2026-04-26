use tokio::sync::MutexGuard;

use crate::{PlaywrightBridge, PlaywrightBridgeError, SharedBridge};

#[derive(Debug, Clone)]
pub struct BrowserContext {
    bridge: SharedBridge,
    page_index: usize,
    current_url: Option<String>,
    browser_has_url: Option<String>,
}

impl BrowserContext {
    #[must_use]
    pub fn new(bridge: SharedBridge) -> Self {
        Self::new_shared(bridge, 0)
    }

    #[must_use]
    pub fn new_shared(bridge: SharedBridge, page_index: usize) -> Self {
        Self {
            bridge,
            page_index,
            current_url: None,
            browser_has_url: None,
        }
    }

    #[must_use]
    pub fn bridge(&self) -> &SharedBridge {
        &self.bridge
    }

    pub async fn acquire_bridge(
        &mut self,
    ) -> Result<MutexGuard<'_, PlaywrightBridge>, PlaywrightBridgeError> {
        let needs_navigate = match (&self.current_url, &self.browser_has_url) {
            (Some(current), Some(loaded)) => current != loaded,
            (Some(_), None) => true,
            _ => false,
        };

        if needs_navigate {
            if let Some(url) = self.current_url.clone() {
                let page_idx = i64::try_from(self.page_index).unwrap_or(0);
                let mut guard = self.bridge.lock().await;
                guard.switch_tab(page_idx).await?;
                guard.navigate(&url).await?;
                drop(guard);
                self.browser_has_url = Some(url);
            }
        }

        let page_idx = i64::try_from(self.page_index).unwrap_or(0);
        let mut guard = self.bridge.lock().await;
        guard.switch_tab(page_idx).await?;
        Ok(guard)
    }

    pub fn set_navigated_url(&mut self, url: &str, loaded_in_browser: bool) {
        self.current_url = Some(url.to_string());
        if loaded_in_browser {
            self.browser_has_url = Some(url.to_string());
        }
    }

    #[must_use]
    pub fn page_index(&self) -> usize {
        self.page_index
    }

    pub fn set_page_index(&mut self, page_index: usize) {
        self.page_index = page_index;
    }
}
