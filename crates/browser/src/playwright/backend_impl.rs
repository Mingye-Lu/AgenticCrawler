use crate::browser_backend::BrowserBackend;

use super::bridge::PlaywrightBridge;
use super::types::{BridgeError, BrowserState, PageInfo};

#[async_trait::async_trait]
impl BrowserBackend for PlaywrightBridge {
    async fn navigate(&mut self, url: &str) -> Result<PageInfo, BridgeError> {
        PlaywrightBridge::navigate(self, url).await
    }

    async fn new_page(&mut self, url: Option<&str>) -> Result<usize, BridgeError> {
        PlaywrightBridge::new_page(self, url).await
    }

    async fn close_page(&mut self, page_index: usize) -> Result<(), BridgeError> {
        PlaywrightBridge::close_page(self, page_index).await
    }

    async fn scroll(&mut self, direction: &str, pixels: i64) -> Result<(), BridgeError> {
        PlaywrightBridge::scroll(self, direction, pixels).await
    }

    async fn page_map(&mut self) -> Result<serde_json::Value, BridgeError> {
        PlaywrightBridge::page_map(self).await
    }

    async fn read_content(
        &mut self,
        heading: Option<&str>,
        selector: Option<&str>,
        offset: usize,
        max_chars: usize,
    ) -> Result<serde_json::Value, BridgeError> {
        PlaywrightBridge::read_content(self, heading, selector, offset, max_chars).await
    }

    async fn wait_for_selector(
        &mut self,
        selector: &str,
        timeout_ms: u64,
    ) -> Result<bool, BridgeError> {
        PlaywrightBridge::wait_for_selector(self, selector, timeout_ms).await
    }

    async fn select_option(&mut self, selector: &str, value: &str) -> Result<(), BridgeError> {
        PlaywrightBridge::select_option(self, selector, value).await
    }

    async fn evaluate(&mut self, script: &str) -> Result<serde_json::Value, BridgeError> {
        PlaywrightBridge::evaluate(self, script).await
    }

    async fn hover(&mut self, selector: &str) -> Result<(), BridgeError> {
        PlaywrightBridge::hover(self, selector).await
    }

    async fn press_key(&mut self, key: &str, selector: Option<&str>) -> Result<(), BridgeError> {
        PlaywrightBridge::press_key(self, key, selector).await
    }

    async fn switch_tab(&mut self, index: i64) -> Result<serde_json::Value, BridgeError> {
        PlaywrightBridge::switch_tab(self, index).await
    }

    async fn export_cookies(&mut self) -> Result<BrowserState, BridgeError> {
        PlaywrightBridge::export_cookies(self).await
    }

    async fn import_cookies(&mut self, state: &BrowserState) -> Result<(), BridgeError> {
        PlaywrightBridge::import_cookies(self, state).await
    }

    async fn import_cookies_only(&mut self, state: &BrowserState) -> Result<(), BridgeError> {
        PlaywrightBridge::import_cookies_only(self, state).await
    }

    async fn import_local_storage(&mut self, state: &BrowserState) -> Result<(), BridgeError> {
        PlaywrightBridge::import_local_storage(self, state).await
    }

    async fn list_resources(&mut self) -> Result<serde_json::Value, BridgeError> {
        PlaywrightBridge::list_resources(self).await
    }

    async fn save_file(&mut self, url: &str, path: &str) -> Result<String, BridgeError> {
        PlaywrightBridge::save_file(self, url, path).await
    }

    async fn click(&mut self, selector: &str) -> Result<(), BridgeError> {
        PlaywrightBridge::click(self, selector).await
    }

    async fn click_at(&mut self, x: f64, y: f64) -> Result<(), BridgeError> {
        PlaywrightBridge::click_at(self, x, y).await
    }

    async fn fill(&mut self, selector: &str, value: &str) -> Result<(), BridgeError> {
        PlaywrightBridge::fill(self, selector, value).await
    }

    async fn screenshot(
        &mut self,
        selector: Option<&str>,
    ) -> Result<(String, usize), BridgeError> {
        PlaywrightBridge::screenshot(self, selector).await
    }

    async fn go_back(&mut self) -> Result<String, BridgeError> {
        PlaywrightBridge::go_back(self).await
    }
}
