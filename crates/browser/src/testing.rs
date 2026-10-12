use std::collections::BTreeMap;

use async_trait::async_trait;
use serde_json::Value;

use crate::{BridgeError, BrowserBackend, BrowserState, PageInfo, ScreenshotOptions};

#[derive(Debug, Default)]
pub struct NopBridge;

#[async_trait]
impl BrowserBackend for NopBridge {
    async fn navigate(&mut self, _url: &str) -> Result<PageInfo, BridgeError> {
        Err(BridgeError::Protocol("NopBridge".into()))
    }
    async fn new_page(&mut self, _url: Option<&str>) -> Result<usize, BridgeError> {
        Err(BridgeError::Protocol("NopBridge".into()))
    }
    async fn close_page(&mut self, _page_index: usize) -> Result<(), BridgeError> {
        Err(BridgeError::Protocol("NopBridge".into()))
    }
    async fn scroll(
        &mut self,
        _direction: &str,
        _pixels: i64,
        _selector: Option<&str>,
    ) -> Result<(), BridgeError> {
        Err(BridgeError::Protocol("NopBridge".into()))
    }
    async fn page_map(
        &mut self,
        _scope: Option<&str>,
        _compound_enrichment: bool,
        _depth: Option<usize>,
    ) -> Result<Value, BridgeError> {
        Err(BridgeError::Protocol("NopBridge".into()))
    }
    async fn read_content(
        &mut self,
        _heading: Option<&str>,
        _selector: Option<&str>,
        _offset: usize,
        _max_chars: usize,
    ) -> Result<Value, BridgeError> {
        Err(BridgeError::Protocol("NopBridge".into()))
    }
    async fn wait_for_selector(
        &mut self,
        _selector: &str,
        _timeout_ms: u64,
        _state: Option<&str>,
    ) -> Result<bool, BridgeError> {
        Err(BridgeError::Protocol("NopBridge".into()))
    }
    async fn select_option(&mut self, _selector: &str, _value: &str) -> Result<(), BridgeError> {
        Err(BridgeError::Protocol("NopBridge".into()))
    }
    async fn evaluate(&mut self, _script: &str) -> Result<Value, BridgeError> {
        Err(BridgeError::Protocol("NopBridge".into()))
    }
    async fn hover(&mut self, _selector: &str) -> Result<(), BridgeError> {
        Err(BridgeError::Protocol("NopBridge".into()))
    }
    async fn press_key(&mut self, _key: &str, _selector: Option<&str>) -> Result<(), BridgeError> {
        Err(BridgeError::Protocol("NopBridge".into()))
    }
    async fn switch_tab(&mut self, _index: i64) -> Result<Value, BridgeError> {
        Err(BridgeError::Protocol("NopBridge".into()))
    }
    async fn export_cookies(&mut self) -> Result<BrowserState, BridgeError> {
        Err(BridgeError::Protocol("NopBridge".into()))
    }
    async fn import_cookies(&mut self, _state: &BrowserState) -> Result<(), BridgeError> {
        Err(BridgeError::Protocol("NopBridge".into()))
    }
    async fn import_cookies_only(&mut self, _state: &BrowserState) -> Result<(), BridgeError> {
        Err(BridgeError::Protocol("NopBridge".into()))
    }
    async fn import_local_storage(&mut self, _state: &BrowserState) -> Result<(), BridgeError> {
        Err(BridgeError::Protocol("NopBridge".into()))
    }
    async fn list_resources(&mut self) -> Result<Value, BridgeError> {
        Err(BridgeError::Protocol("NopBridge".into()))
    }
    async fn save_file(
        &mut self,
        _url: &str,
        _path: &str,
        _headers: Option<&BTreeMap<String, String>>,
    ) -> Result<String, BridgeError> {
        Err(BridgeError::Protocol("NopBridge".into()))
    }
    async fn click(&mut self, _selector: &str) -> Result<(), BridgeError> {
        Err(BridgeError::Protocol("NopBridge".into()))
    }
    async fn click_at(&mut self, _x: f64, _y: f64) -> Result<(), BridgeError> {
        Err(BridgeError::Protocol("NopBridge".into()))
    }
    async fn fill(&mut self, _selector: &str, _value: &str) -> Result<(), BridgeError> {
        Err(BridgeError::Protocol("NopBridge".into()))
    }
    async fn screenshot(
        &mut self,
        _options: &ScreenshotOptions<'_>,
    ) -> Result<(String, usize), BridgeError> {
        Err(BridgeError::Protocol("NopBridge".into()))
    }
    async fn go_back(&mut self) -> Result<String, BridgeError> {
        Err(BridgeError::Protocol("NopBridge".into()))
    }
    async fn set_device(&mut self, _options: &Value) -> Result<Value, BridgeError> {
        Err(BridgeError::Protocol("NopBridge".into()))
    }
    async fn poll_observations(&mut self) -> Result<Vec<crate::ObservationEvent>, BridgeError> {
        Ok(Vec::new())
    }
    async fn set_seq(&mut self, _seq: u64) -> Result<(), BridgeError> {
        Ok(())
    }
}
