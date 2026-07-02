//! Shared test backend that replays observation events through the
//! `BrowserBackend` trait object, exactly as the real bridges do. Lets the
//! observation tools be exercised over the full `poll_observations` dispatch
//! path (the path that regressed in the inherent-vs-trait wiring bug).

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use browser::{
    BridgeError, BrowserBackend, BrowserContext, BrowserState, ObservationEvent, PageInfo,
    ScreenshotOptions, SharedBridge, StorageEntry, StorageType,
};
use serde_json::{json, Value};
use tokio::sync::Mutex;

pub type SaveFileHeadersRecord = Arc<Mutex<Option<BTreeMap<String, String>>>>;

#[derive(Debug, Default)]
pub struct ObservationMockBackend {
    pub observations: Vec<ObservationEvent>,
    pub last_save_file_headers: Option<BTreeMap<String, String>>,
    pub save_file_headers_sink: Option<SaveFileHeadersRecord>,
}

#[async_trait]
impl BrowserBackend for ObservationMockBackend {
    async fn poll_observations(&mut self) -> Result<Vec<ObservationEvent>, BridgeError> {
        Ok(std::mem::take(&mut self.observations))
    }
    async fn set_seq(&mut self, _seq: u64) -> Result<(), BridgeError> {
        Ok(())
    }
    async fn navigate(&mut self, _: &str) -> Result<PageInfo, BridgeError> {
        Err(BridgeError::Protocol("unused".to_string()))
    }
    async fn new_page(&mut self, _: Option<&str>) -> Result<usize, BridgeError> {
        Ok(0)
    }
    async fn close_page(&mut self, _: usize) -> Result<(), BridgeError> {
        Ok(())
    }
    async fn scroll(&mut self, _: &str, _: i64) -> Result<(), BridgeError> {
        Ok(())
    }
    async fn page_map(
        &mut self,
        _: Option<&str>,
        _: bool,
        _: Option<usize>,
    ) -> Result<Value, BridgeError> {
        Ok(json!({}))
    }
    async fn read_content(
        &mut self,
        _: Option<&str>,
        _: Option<&str>,
        _: usize,
        _: usize,
    ) -> Result<Value, BridgeError> {
        Ok(json!({}))
    }
    async fn wait_for_selector(
        &mut self,
        _: &str,
        _: u64,
        _: Option<&str>,
    ) -> Result<bool, BridgeError> {
        Ok(true)
    }
    async fn select_option(&mut self, _: &str, _: &str) -> Result<(), BridgeError> {
        Ok(())
    }
    async fn evaluate(&mut self, _: &str) -> Result<Value, BridgeError> {
        Ok(Value::Null)
    }
    async fn hover(&mut self, _: &str) -> Result<(), BridgeError> {
        Ok(())
    }
    async fn press_key(&mut self, _: &str, _: Option<&str>) -> Result<(), BridgeError> {
        Ok(())
    }
    async fn switch_tab(&mut self, _: i64) -> Result<Value, BridgeError> {
        Ok(json!({}))
    }
    async fn export_cookies(&mut self) -> Result<BrowserState, BridgeError> {
        Ok(BrowserState {
            cookies: Value::Array(vec![]),
            local_storage: Value::Object(serde_json::Map::new()),
            url: String::new(),
        })
    }
    async fn import_cookies(&mut self, _: &BrowserState) -> Result<(), BridgeError> {
        Ok(())
    }
    async fn import_cookies_only(&mut self, _: &BrowserState) -> Result<(), BridgeError> {
        Ok(())
    }
    async fn import_local_storage(&mut self, _: &BrowserState) -> Result<(), BridgeError> {
        Ok(())
    }
    async fn list_resources(&mut self) -> Result<Value, BridgeError> {
        Ok(json!([]))
    }
    async fn save_file(
        &mut self,
        _: &str,
        _: &str,
        headers: Option<&BTreeMap<String, String>>,
    ) -> Result<String, BridgeError> {
        self.last_save_file_headers = headers.cloned();
        if let Some(sink) = &self.save_file_headers_sink {
            *sink.lock().await = self.last_save_file_headers.clone();
        }
        Ok(String::new())
    }
    async fn click(&mut self, _: &str) -> Result<(), BridgeError> {
        Ok(())
    }
    async fn click_at(&mut self, _: f64, _: f64) -> Result<(), BridgeError> {
        Ok(())
    }
    async fn fill(&mut self, _: &str, _: &str) -> Result<(), BridgeError> {
        Ok(())
    }
    async fn screenshot(
        &mut self,
        _: &ScreenshotOptions<'_>,
    ) -> Result<(String, usize), BridgeError> {
        Ok((String::new(), 0))
    }
    async fn go_back(&mut self) -> Result<String, BridgeError> {
        Ok(String::new())
    }
    async fn set_device(&mut self, _: &Value) -> Result<Value, BridgeError> {
        Ok(json!({}))
    }
    async fn get_storage(
        &mut self,
        _: StorageType,
    ) -> Result<(Vec<StorageEntry>, Vec<StorageEntry>), BridgeError> {
        Ok((Vec::new(), Vec::new()))
    }
}

#[must_use]
pub fn browser_with_observations(observations: Vec<ObservationEvent>) -> BrowserContext {
    let bridge: SharedBridge = Arc::new(Mutex::new(Box::new(ObservationMockBackend {
        observations,
        last_save_file_headers: None,
        save_file_headers_sink: None,
    }) as Box<dyn BrowserBackend + Send>));
    BrowserContext::new(bridge)
}

#[must_use]
pub fn browser_with_save_file_header_recorder(
    observations: Vec<ObservationEvent>,
) -> (BrowserContext, SaveFileHeadersRecord) {
    let sink = Arc::new(Mutex::new(None));
    let bridge: SharedBridge = Arc::new(Mutex::new(Box::new(ObservationMockBackend {
        observations,
        last_save_file_headers: None,
        save_file_headers_sink: Some(sink.clone()),
    }) as Box<dyn BrowserBackend + Send>));
    (BrowserContext::new(bridge), sink)
}

pub async fn take_recorded_save_file_headers(
    sink: &SaveFileHeadersRecord,
) -> Option<BTreeMap<String, String>> {
    sink.lock().await.clone()
}
