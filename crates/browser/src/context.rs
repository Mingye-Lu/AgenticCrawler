use serde_json::Value;
use tokio::sync::MutexGuard;

use crate::ref_map::RefMap;
use crate::{BridgeError, BrowserBackend, SharedBridge};

/// Normalize a URL for snapshot storage: strips fragment unless it's a hash-based route.
fn normalize_url_for_snapshot(url: &str) -> String {
    match url.split_once('#') {
        Some((_, frag)) if frag.starts_with('/') || frag.starts_with("!/") => url.to_string(),
        Some((base, _)) => base.to_string(),
        None => url.to_string(),
    }
}

#[derive(Debug, Clone)]
pub struct BrowserContext {
    bridge: SharedBridge,
    page_index: usize,
    current_url: Option<String>,
    browser_has_url: Option<String>,
    /// Cached `page_map` snapshots, keyed by (`normalized_url`, `scope_key`).
    /// `scope_key` is the scope string or "" for `None`. Bounded to 8 entries (LRU/insertion-order eviction).
    /// Used for differential comparison on subsequent same-page interactions.
    page_snapshots: Vec<((String, String), Value)>,
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
            page_snapshots: Vec::new(),
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

    #[must_use]
    pub fn current_url(&self) -> Option<&str> {
        self.current_url.as_deref()
    }

    pub fn set_page_index(&mut self, page_index: usize) {
        self.page_index = page_index;
    }

    pub fn set_page_snapshot(&mut self, url: &str, scope: Option<&str>, page_map: Value) {
        let normalized_url = normalize_url_for_snapshot(url);
        let scope_key = scope.unwrap_or("").to_string();
        let key = (normalized_url, scope_key);

        self.page_snapshots.retain(|(k, _)| k != &key);
        self.page_snapshots.push((key, page_map));

        if self.page_snapshots.len() > 8 {
            self.page_snapshots.remove(0);
        }
    }

    #[must_use]
    pub fn page_snapshot_for_url(&self, url: &str, scope: Option<&str>) -> Option<&Value> {
        let normalized_url = normalize_url_for_snapshot(url);
        let scope_key = scope.unwrap_or("").to_string();
        let key = (normalized_url, scope_key);

        self.page_snapshots
            .iter()
            .find(|(k, _)| k == &key)
            .map(|(_, map)| map)
    }

    pub fn clear_page_snapshot(&mut self) {
        self.page_snapshots.clear();
    }

    #[must_use]
    pub fn snapshot_url(&self) -> Option<&str> {
        self.page_snapshots.last().map(|((url, _), _)| url.as_str())
    }

    #[must_use]
    pub fn last_page_snapshot(&self) -> Option<&Value> {
        self.page_snapshots.last().map(|(_, snapshot)| snapshot)
    }

    pub fn ref_map_mut(&mut self) -> &mut RefMap {
        &mut self.ref_map
    }

    #[must_use]
    pub fn ref_map(&self) -> &RefMap {
        &self.ref_map
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::NopBridge;
    use serde_json::json;

    fn test_bridge() -> SharedBridge {
        std::sync::Arc::new(tokio::sync::Mutex::new(
            Box::new(NopBridge) as Box<dyn BrowserBackend + Send>
        ))
    }

    #[test]
    fn snapshot_store_retrieve_with_scope_none() {
        let mut ctx = BrowserContext::new(test_bridge());
        let value = json!({"test": "data"});

        ctx.set_page_snapshot("https://example.com", None, value.clone());

        let retrieved = ctx.page_snapshot_for_url("https://example.com", None);
        assert_eq!(retrieved, Some(&value));
    }

    #[test]
    fn snapshot_store_retrieve_with_scope() {
        let mut ctx = BrowserContext::new(test_bridge());
        let value1 = json!({"scope": "dialog"});
        let value2 = json!({"scope": "main"});

        ctx.set_page_snapshot("https://example.com", Some("dialog"), value1.clone());
        ctx.set_page_snapshot("https://example.com", Some("main"), value2.clone());

        assert_eq!(
            ctx.page_snapshot_for_url("https://example.com", Some("dialog")),
            Some(&value1)
        );
        assert_eq!(
            ctx.page_snapshot_for_url("https://example.com", Some("main")),
            Some(&value2)
        );
        assert_eq!(ctx.page_snapshot_for_url("https://example.com", None), None);
    }

    #[test]
    fn snapshot_different_scopes_independent() {
        let mut ctx = BrowserContext::new(test_bridge());
        let value_dialog = json!({"type": "dialog"});
        let value_none = json!({"type": "none"});

        ctx.set_page_snapshot("https://example.com", Some("dialog"), value_dialog.clone());
        ctx.set_page_snapshot("https://example.com", None, value_none.clone());

        assert_eq!(
            ctx.page_snapshot_for_url("https://example.com", Some("dialog")),
            Some(&value_dialog)
        );
        assert_eq!(
            ctx.page_snapshot_for_url("https://example.com", None),
            Some(&value_none)
        );
    }

    #[test]
    fn snapshot_eviction_at_8_entries() {
        let mut ctx = BrowserContext::new(test_bridge());

        for i in 0..9 {
            let url = format!("https://example.com/page{i}");
            let value = json!({"page": i});
            ctx.set_page_snapshot(&url, None, value);
        }

        assert_eq!(ctx.page_snapshots.len(), 8);
        assert_eq!(
            ctx.page_snapshot_for_url("https://example.com/page0", None),
            None
        );
        assert!(ctx
            .page_snapshot_for_url("https://example.com/page8", None)
            .is_some());
    }

    #[test]
    fn snapshot_url_returns_most_recent() {
        let mut ctx = BrowserContext::new(test_bridge());

        ctx.set_page_snapshot("https://example.com/page1", None, json!({}));
        ctx.set_page_snapshot("https://example.com/page2", None, json!({}));

        assert_eq!(ctx.snapshot_url(), Some("https://example.com/page2"));
    }

    #[test]
    fn snapshot_normalize_url_strips_fragment() {
        let mut ctx = BrowserContext::new(test_bridge());
        let value = json!({"test": "data"});

        ctx.set_page_snapshot("https://example.com#section", None, value.clone());

        let retrieved = ctx.page_snapshot_for_url("https://example.com", None);
        assert_eq!(retrieved, Some(&value));
    }

    #[test]
    fn snapshot_normalize_url_preserves_hash_routes() {
        let mut ctx = BrowserContext::new(test_bridge());
        let value1 = json!({"route": "home"});
        let value2 = json!({"route": "about"});

        ctx.set_page_snapshot("https://example.com#/home", None, value1.clone());
        ctx.set_page_snapshot("https://example.com#/about", None, value2.clone());

        assert_eq!(
            ctx.page_snapshot_for_url("https://example.com#/home", None),
            Some(&value1)
        );
        assert_eq!(
            ctx.page_snapshot_for_url("https://example.com#/about", None),
            Some(&value2)
        );
    }

    #[test]
    fn snapshot_clear_removes_all() {
        let mut ctx = BrowserContext::new(test_bridge());

        ctx.set_page_snapshot("https://example.com", None, json!({}));
        ctx.set_page_snapshot("https://example.com", Some("dialog"), json!({}));

        ctx.clear_page_snapshot();

        assert_eq!(ctx.page_snapshots.len(), 0);
        assert_eq!(ctx.snapshot_url(), None);
    }
}
