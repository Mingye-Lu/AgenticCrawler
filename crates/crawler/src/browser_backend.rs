use std::fmt::Debug;

use async_trait::async_trait;

use crate::{BrowserState, PageInfo, PlaywrightBridgeError};

#[async_trait]
pub trait BrowserBackend: Debug {
    async fn navigate(&mut self, url: &str) -> Result<PageInfo, PlaywrightBridgeError>;
    async fn new_page(&mut self, url: Option<&str>) -> Result<usize, PlaywrightBridgeError>;
    async fn close_page(&mut self, page_index: usize) -> Result<(), PlaywrightBridgeError>;
    async fn scroll(&mut self, direction: &str, pixels: i64) -> Result<(), PlaywrightBridgeError>;
    async fn page_map(&mut self) -> Result<serde_json::Value, PlaywrightBridgeError>;
    async fn read_content(
        &mut self,
        heading: Option<&str>,
        selector: Option<&str>,
        offset: usize,
        max_chars: usize,
    ) -> Result<serde_json::Value, PlaywrightBridgeError>;
    async fn wait_for_selector(
        &mut self,
        selector: &str,
        timeout_ms: u64,
    ) -> Result<bool, PlaywrightBridgeError>;
    async fn select_option(
        &mut self,
        selector: &str,
        value: &str,
    ) -> Result<(), PlaywrightBridgeError>;
    async fn evaluate(&mut self, script: &str) -> Result<serde_json::Value, PlaywrightBridgeError>;
    async fn hover(&mut self, selector: &str) -> Result<(), PlaywrightBridgeError>;
    async fn press_key(
        &mut self,
        key: &str,
        selector: Option<&str>,
    ) -> Result<(), PlaywrightBridgeError>;
    async fn switch_tab(&mut self, index: i64) -> Result<serde_json::Value, PlaywrightBridgeError>;
    async fn export_cookies(&mut self) -> Result<BrowserState, PlaywrightBridgeError>;
    async fn import_cookies(&mut self, state: &BrowserState) -> Result<(), PlaywrightBridgeError>;
    async fn import_cookies_only(
        &mut self,
        state: &BrowserState,
    ) -> Result<(), PlaywrightBridgeError>;
    async fn import_local_storage(
        &mut self,
        state: &BrowserState,
    ) -> Result<(), PlaywrightBridgeError>;
    async fn list_resources(&mut self) -> Result<serde_json::Value, PlaywrightBridgeError>;
    async fn save_file(&mut self, url: &str, path: &str) -> Result<String, PlaywrightBridgeError>;
    async fn click(&mut self, selector: &str) -> Result<(), PlaywrightBridgeError>;
    async fn fill(&mut self, selector: &str, value: &str) -> Result<(), PlaywrightBridgeError>;
    async fn screenshot(&mut self) -> Result<(String, usize), PlaywrightBridgeError>;
    async fn go_back(&mut self) -> Result<String, PlaywrightBridgeError>;
}
