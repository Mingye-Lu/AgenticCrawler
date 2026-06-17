use std::fmt::Debug;

use async_trait::async_trait;

use crate::{BridgeError, BrowserState, PageInfo};

/// Options for the screenshot backend call.
#[derive(Debug, Clone, Default)]
pub struct ScreenshotOptions<'a> {
    /// CSS selector to screenshot a specific element.
    pub selector: Option<&'a str>,
    /// Image format: "png", "jpeg", or "webp".
    pub format: Option<&'a str>,
    /// Quality for lossy formats (jpeg/webp), 0-100.
    pub quality: Option<u32>,
    /// Capture the full scrollable page.
    pub full_page: bool,
}

#[async_trait]
pub trait BrowserBackend: Debug {
    async fn navigate(&mut self, url: &str) -> Result<PageInfo, BridgeError>;
    async fn new_page(&mut self, url: Option<&str>) -> Result<usize, BridgeError>;
    async fn close_page(&mut self, page_index: usize) -> Result<(), BridgeError>;
    async fn scroll(&mut self, direction: &str, pixels: i64) -> Result<(), BridgeError>;
    async fn page_map(
        &mut self,
        scope: Option<&str>,
        compound_enrichment: bool,
    ) -> Result<serde_json::Value, BridgeError>;
    async fn read_content(
        &mut self,
        heading: Option<&str>,
        selector: Option<&str>,
        offset: usize,
        max_chars: usize,
    ) -> Result<serde_json::Value, BridgeError>;
    async fn wait_for_selector(
        &mut self,
        selector: &str,
        timeout_ms: u64,
        state: Option<&str>,
    ) -> Result<bool, BridgeError>;
    async fn select_option(&mut self, selector: &str, value: &str) -> Result<(), BridgeError>;
    async fn evaluate(&mut self, script: &str) -> Result<serde_json::Value, BridgeError>;
    async fn hover(&mut self, selector: &str) -> Result<(), BridgeError>;
    async fn press_key(&mut self, key: &str, selector: Option<&str>) -> Result<(), BridgeError>;
    async fn switch_tab(&mut self, index: i64) -> Result<serde_json::Value, BridgeError>;
    async fn export_cookies(&mut self) -> Result<BrowserState, BridgeError>;
    async fn import_cookies(&mut self, state: &BrowserState) -> Result<(), BridgeError>;
    async fn import_cookies_only(&mut self, state: &BrowserState) -> Result<(), BridgeError>;
    async fn import_local_storage(&mut self, state: &BrowserState) -> Result<(), BridgeError>;
    async fn list_resources(&mut self) -> Result<serde_json::Value, BridgeError>;
    async fn save_file(&mut self, url: &str, path: &str) -> Result<String, BridgeError>;
    async fn click(&mut self, selector: &str) -> Result<(), BridgeError>;
    async fn click_at(&mut self, x: f64, y: f64) -> Result<(), BridgeError>;
    async fn fill(&mut self, selector: &str, value: &str) -> Result<(), BridgeError>;
    async fn screenshot(
        &mut self,
        options: &ScreenshotOptions<'_>,
    ) -> Result<(String, usize), BridgeError>;
    async fn go_back(&mut self) -> Result<String, BridgeError>;
    async fn set_device(
        &mut self,
        options: &serde_json::Value,
    ) -> Result<serde_json::Value, BridgeError>;

    async fn page_map_feedback(&mut self) -> Result<serde_json::Value, BridgeError> {
        self.page_map(None, false).await
    }
}
