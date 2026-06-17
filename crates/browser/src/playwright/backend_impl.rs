use crate::browser_backend::{BrowserBackend, ScreenshotOptions};
use crate::{ConsoleMessageEvent, NetworkRequestEvent, ObservationEvent, WebSocketFrameEvent};

use super::bridge::PlaywrightBridge;
use super::types::{BridgeError, BrowserState, PageInfo};

impl PlaywrightBridge {
    pub async fn poll_observations(&mut self) -> Result<Vec<ObservationEvent>, BridgeError> {
        let result = self
            .send_raw_command(&serde_json::json!({
                "action": "poll_observations",
            }))
            .await?;
        let events = result
            .get("events")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default();

        events
            .into_iter()
            .map(|event| {
                let event_type = event
                    .get("type")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| {
                        BridgeError::Protocol("observation event missing type".to_string())
                    })?;

                match event_type {
                    "NetworkRequest" => serde_json::from_value::<NetworkRequestEvent>(event)
                        .map(ObservationEvent::NetworkRequest)
                        .map_err(|error| {
                            BridgeError::Protocol(format!(
                                "failed to parse network observation event: {error}"
                            ))
                        }),
                    "ConsoleMessage" => serde_json::from_value::<ConsoleMessageEvent>(event)
                        .map(ObservationEvent::ConsoleMessage)
                        .map_err(|error| {
                            BridgeError::Protocol(format!(
                                "failed to parse console observation event: {error}"
                            ))
                        }),
                    "WebSocketFrame" => serde_json::from_value::<WebSocketFrameEvent>(event)
                        .map(ObservationEvent::WebSocketFrame)
                        .map_err(|error| {
                            BridgeError::Protocol(format!(
                                "failed to parse websocket observation event: {error}"
                            ))
                        }),
                    other => Err(BridgeError::Protocol(format!(
                        "unsupported observation event type: {other}"
                    ))),
                }
            })
            .collect()
    }

    pub async fn set_seq(&mut self, seq: u64) -> Result<(), BridgeError> {
        self.set_seq_raw(seq).await
    }
}

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

    async fn page_map(
        &mut self,
        scope: Option<&str>,
        compound_enrichment: bool,
    ) -> Result<serde_json::Value, BridgeError> {
        PlaywrightBridge::page_map(self, scope, compound_enrichment).await
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
        state: Option<&str>,
    ) -> Result<bool, BridgeError> {
        PlaywrightBridge::wait_for_selector(self, selector, timeout_ms, state).await
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
        options: &ScreenshotOptions<'_>,
    ) -> Result<(String, usize), BridgeError> {
        PlaywrightBridge::screenshot(self, options).await
    }

    async fn go_back(&mut self) -> Result<String, BridgeError> {
        PlaywrightBridge::go_back(self).await
    }

    async fn reload(&mut self) -> Result<PageInfo, BridgeError> {
        PlaywrightBridge::reload(self).await
    }

    async fn set_device(
        &mut self,
        options: &serde_json::Value,
    ) -> Result<serde_json::Value, BridgeError> {
        PlaywrightBridge::set_device(self, options).await
    }

    async fn get_cookies(&mut self) -> Result<Vec<crate::CookieInfo>, BridgeError> {
        PlaywrightBridge::get_cookies(self).await
    }

    async fn get_storage(
        &mut self,
        storage_type: crate::StorageType,
    ) -> Result<(Vec<crate::StorageEntry>, Vec<crate::StorageEntry>), BridgeError> {
        PlaywrightBridge::get_storage(self, storage_type).await
    }

    async fn start_coverage(&mut self, js: bool, css: bool) -> Result<(), BridgeError> {
        PlaywrightBridge::start_coverage(self, js, css).await
    }

    async fn stop_coverage(&mut self) -> Result<crate::CoverageData, BridgeError> {
        PlaywrightBridge::stop_coverage(self).await
    }
}
