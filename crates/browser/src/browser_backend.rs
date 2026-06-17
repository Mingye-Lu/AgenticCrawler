use std::collections::HashMap;
use std::fmt::Debug;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::observation::ObservationEvent;
use crate::{BridgeError, BrowserState, PageInfo};

#[derive(Debug, Clone, Default)]
pub struct ScreenshotOptions<'a> {
    pub selector: Option<&'a str>,
    pub format: Option<&'a str>,
    pub quality: Option<u32>,
    pub full_page: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CookieInfo {
    pub name: String,
    pub value: String,
    pub domain: String,
    pub path: String,
    pub expires: Option<f64>,
    pub secure: bool,
    pub http_only: bool,
    pub same_site: Option<String>,
    pub size_bytes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageEntry {
    pub key: String,
    pub value: String,
    pub size_bytes: usize,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum StorageType {
    Local,
    Session,
    All,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterceptRule {
    pub pattern: String,
    pub action: InterceptAction,
    pub mock: Option<MockResponse>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum InterceptAction {
    Block,
    MockResponse,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MockResponse {
    pub status: u16,
    pub headers: Option<HashMap<String, String>>,
    pub body: Option<String>,
    pub content_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoverageData {
    pub js_coverage: Vec<FileCoverage>,
    pub css_coverage: Vec<FileCoverage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileCoverage {
    pub url: String,
    pub total_bytes: usize,
    pub used_bytes: usize,
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

    async fn poll_observations(&mut self) -> Result<Vec<ObservationEvent>, BridgeError> {
        Err(BridgeError::Unsupported(
            "poll_observations not implemented for this backend".into(),
        ))
    }

    async fn set_seq(&mut self, _seq: u64) -> Result<(), BridgeError> {
        Err(BridgeError::Unsupported(
            "set_seq not implemented for this backend".into(),
        ))
    }

    async fn reload(&mut self) -> Result<PageInfo, BridgeError> {
        Err(BridgeError::Unsupported(
            "reload not implemented for this backend".into(),
        ))
    }

    async fn get_cookies(&mut self) -> Result<Vec<CookieInfo>, BridgeError> {
        Err(BridgeError::Unsupported(
            "get_cookies not implemented for this backend".into(),
        ))
    }

    async fn get_storage(
        &mut self,
        _storage_type: StorageType,
    ) -> Result<(Vec<StorageEntry>, Vec<StorageEntry>), BridgeError> {
        Err(BridgeError::Unsupported(
            "get_storage not implemented for this backend".into(),
        ))
    }

    async fn start_coverage(&mut self, _js: bool, _css: bool) -> Result<(), BridgeError> {
        Err(BridgeError::Unsupported(
            "start_coverage not implemented for this backend".into(),
        ))
    }

    async fn stop_coverage(&mut self) -> Result<CoverageData, BridgeError> {
        Err(BridgeError::Unsupported(
            "stop_coverage not implemented for this backend".into(),
        ))
    }

    async fn add_intercept_rule(&mut self, _rule: InterceptRule) -> Result<String, BridgeError> {
        Err(BridgeError::Unsupported(
            "add_intercept_rule not implemented for this backend".into(),
        ))
    }

    async fn remove_intercept_rule(&mut self, _rule_id: &str) -> Result<(), BridgeError> {
        Err(BridgeError::Unsupported(
            "remove_intercept_rule not implemented for this backend".into(),
        ))
    }

    async fn clear_intercept_rules(&mut self) -> Result<(), BridgeError> {
        Err(BridgeError::Unsupported(
            "clear_intercept_rules not implemented for this backend".into(),
        ))
    }
}
