use serde_json::Value;
use tokio::sync::MutexGuard;

use crate::ref_map::RefMap;
use crate::{BridgeError, BrowserBackend, SharedBridge};

#[derive(Debug, Clone)]
pub struct BrowserContext {
    bridge: SharedBridge,
    page_index: usize,
    current_url: Option<String>,
    browser_has_url: Option<String>,
    /// Cached `page_map` from the last post-action feedback, keyed by URL.
    /// Used for differential comparison on subsequent same-page interactions.
    last_page_snapshot: Option<(String, Value)>,
    ref_map: RefMap,
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
            last_page_snapshot: None,
            ref_map: RefMap::new(),
        }
    }

    #[must_use]
    pub fn bridge(&self) -> &SharedBridge {
        &self.bridge
    }

    pub async fn acquire_bridge(
        &mut self,
    ) -> Result<MutexGuard<'_, Box<dyn BrowserBackend + Send>>, BridgeError> {
        let needs_navigate = match (&self.current_url, &self.browser_has_url) {
            (Some(current), Some(loaded)) => current != loaded,
            (Some(_), None) => true,
            _ => false,
        };

        let page_idx = i64::try_from(self.page_index).map_err(|_| {
            BridgeError::Protocol(format!("page index {} out of range", self.page_index))
        })?;
        let mut guard = self.bridge.lock().await;

        if guard.switch_tab(page_idx).await.is_err() {
            let new_page_index = guard.new_page(None).await?;
            self.page_index = new_page_index;
            let new_page_idx = i64::try_from(new_page_index).map_err(|_| {
                BridgeError::Protocol(format!("page index {new_page_index} out of range"))
            })?;
            guard.switch_tab(new_page_idx).await?;
        }

        if needs_navigate {
            if let Some(url) = self.current_url.clone() {
                guard.navigate(&url).await?;
                self.browser_has_url = Some(url);
            }
        }

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

    pub fn set_page_snapshot(&mut self, url: String, page_map: Value) {
        self.last_page_snapshot = Some((url, page_map));
    }

    #[must_use]
    pub fn page_snapshot_for_url(&self, url: &str) -> Option<&Value> {
        self.last_page_snapshot
            .as_ref()
            .filter(|(cached_url, _)| cached_url == url)
            .map(|(_, map)| map)
    }

    pub fn clear_page_snapshot(&mut self) {
        self.last_page_snapshot = None;
    }

    pub fn ref_map_mut(&mut self) -> &mut RefMap {
        &mut self.ref_map
    }

    pub fn ref_map(&self) -> &RefMap {
        &self.ref_map
    }
}
