use std::collections::BTreeMap;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine;
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot, watch};

use crate::observation::{
    ConsoleMessageEvent, NetworkRequestEvent, ObservationEvent, WebSocketFrameEvent,
};
use crate::ws_server::{BridgeCommand, BridgeResponse};
use crate::{BridgeError, BrowserBackend, BrowserState, PageInfo};

#[cfg(not(test))]
const EXTENSION_COMMAND_TIMEOUT: Duration = Duration::from_mins(1);
#[cfg(test)]
const EXTENSION_COMMAND_TIMEOUT: Duration = Duration::from_millis(50);

pub struct ExtensionBridge {
    command_tx: mpsc::Sender<(BridgeCommand, oneshot::Sender<BridgeResponse>)>,
    connected: watch::Receiver<bool>,
    next_id: Arc<AtomicU64>,
}

impl ExtensionBridge {
    #[must_use]
    pub fn new(
        command_tx: mpsc::Sender<(BridgeCommand, oneshot::Sender<BridgeResponse>)>,
        connected: watch::Receiver<bool>,
    ) -> Self {
        Self {
            command_tx,
            connected,
            next_id: Arc::new(AtomicU64::new(1)),
        }
    }

    async fn send_command(
        &self,
        action: &str,
        payload: Value,
    ) -> Result<BridgeResponse, BridgeError> {
        self.send_command_with_timeout(action, payload, EXTENSION_COMMAND_TIMEOUT)
            .await
    }

    async fn send_command_with_timeout(
        &self,
        action: &str,
        payload: Value,
        timeout: Duration,
    ) -> Result<BridgeResponse, BridgeError> {
        if !*self.connected.borrow() {
            return Err(BridgeError::ExtensionDisconnected);
        }

        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let cmd = BridgeCommand {
            id,
            action: action.to_string(),
            payload,
        };
        let (resp_tx, resp_rx) = oneshot::channel();

        self.command_tx
            .send((cmd, resp_tx))
            .await
            .map_err(|_| BridgeError::ExtensionDisconnected)?;

        tokio::time::timeout(timeout, resp_rx)
            .await
            .map_err(|_| BridgeError::ExtensionTimeout { timeout })?
            .map_err(|_| BridgeError::ExtensionDisconnected)
    }

    fn require_ok(response: BridgeResponse) -> Result<BridgeResponse, BridgeError> {
        if response.ok {
            Ok(response)
        } else {
            Err(BridgeError::Protocol(response.error.unwrap_or_default()))
        }
    }

    fn require_result(response: BridgeResponse, action: &str) -> Result<Value, BridgeError> {
        Self::require_ok(response)?
            .result
            .ok_or_else(|| BridgeError::Protocol(format!("{action} missing result")))
    }

    fn parse_result<T: DeserializeOwned>(result: Value, action: &str) -> Result<T, BridgeError> {
        serde_json::from_value(result)
            .map_err(|error| BridgeError::Protocol(format!("{action} result parse error: {error}")))
    }

    async fn expect_unit(&self, action: &str, payload: Value) -> Result<(), BridgeError> {
        let response = self.send_command(action, payload).await?;
        Self::require_ok(response)?;
        Ok(())
    }

    pub async fn poll_observations(&mut self) -> Result<Vec<ObservationEvent>, BridgeError> {
        let result = self
            .send_command("poll_observations", serde_json::json!({}))
            .await?;
        let events_json = result
            .result
            .and_then(|value| value.get("events").and_then(Value::as_array).cloned())
            .unwrap_or_default();
        let mut events = Vec::new();
        for event_json in events_json {
            if let Ok(event) = parse_observation_event(event_json) {
                events.push(event);
            }
        }
        Ok(events)
    }

    pub async fn set_seq(&mut self, seq: u64) -> Result<(), BridgeError> {
        self.send_command("set_seq", serde_json::json!({ "seq": seq }))
            .await?;
        Ok(())
    }
}

/// Build the `page_map` command payload sent to the extension.
/// `depth` is forwarded so the in-page ARIA walk honors the requested depth.
fn build_page_map_payload(
    scope: Option<&str>,
    compound_enrichment: bool,
    depth: Option<usize>,
) -> Value {
    let mut payload = json!({});
    if let Some(s) = scope {
        payload["scope"] = json!(s);
    }
    if compound_enrichment {
        payload["compoundEnrichment"] = json!(true);
    }
    if let Some(d) = depth {
        payload["depth"] = json!(d);
    }
    payload
}

fn parse_observation_event(json: serde_json::Value) -> Result<ObservationEvent, BridgeError> {
    let event_type = json
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| BridgeError::Protocol("observation event missing type".to_string()))?;

    match event_type {
        "NetworkRequest" => serde_json::from_value::<NetworkRequestEvent>(json)
            .map(|parsed| ObservationEvent::NetworkRequest(Box::new(parsed)))
            .map_err(|error| {
                BridgeError::Protocol(format!("observation NetworkRequest parse error: {error}"))
            }),
        "ConsoleMessage" => serde_json::from_value::<ConsoleMessageEvent>(json)
            .map(ObservationEvent::ConsoleMessage)
            .map_err(|error| {
                BridgeError::Protocol(format!("observation ConsoleMessage parse error: {error}"))
            }),
        "WebSocketFrame" => serde_json::from_value::<WebSocketFrameEvent>(json)
            .map(ObservationEvent::WebSocketFrame)
            .map_err(|error| {
                BridgeError::Protocol(format!("observation WebSocketFrame parse error: {error}"))
            }),
        other => Err(BridgeError::Protocol(format!(
            "unknown observation event type: {other}"
        ))),
    }
}

impl fmt::Debug for ExtensionBridge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExtensionBridge").finish_non_exhaustive()
    }
}

#[async_trait]
impl BrowserBackend for ExtensionBridge {
    async fn poll_observations(&mut self) -> Result<Vec<ObservationEvent>, BridgeError> {
        ExtensionBridge::poll_observations(self).await
    }

    async fn set_seq(&mut self, seq: u64) -> Result<(), BridgeError> {
        ExtensionBridge::set_seq(self, seq).await
    }

    async fn navigate(&mut self, url: &str) -> Result<PageInfo, BridgeError> {
        let response = self.send_command("navigate", json!({ "url": url })).await?;
        let result = Self::require_result(response, "navigate")?;
        Self::parse_result(result, "navigate")
    }

    async fn new_page(&mut self, url: Option<&str>) -> Result<usize, BridgeError> {
        let payload = url.map_or_else(|| json!({}), |url| json!({ "url": url }));
        let response = self.send_command("new_page", payload).await?;
        let result = Self::require_result(response, "new_page")?;
        let page_index = result
            .get("pageIndex")
            .and_then(Value::as_u64)
            .ok_or_else(|| BridgeError::Protocol("new_page missing pageIndex".to_string()))?;
        usize::try_from(page_index).map_err(|_| {
            BridgeError::Protocol(format!(
                "new_page returned out-of-range pageIndex {page_index}"
            ))
        })
    }

    async fn close_page(&mut self, page_index: usize) -> Result<(), BridgeError> {
        self.expect_unit("close_page", json!({ "page_index": page_index }))
            .await
    }

    async fn scroll(
        &mut self,
        direction: &str,
        pixels: i64,
        selector: Option<&str>,
    ) -> Result<(), BridgeError> {
        if let Some(selector) = selector {
            let delta = if direction == "up" { -pixels } else { pixels };
            let selector_json = serde_json::to_string(selector)
                .map_err(|e| BridgeError::Protocol(format!("failed to encode selector: {e}")))?;
            let script = format!(
                "(() => {{ const el = document.querySelector({selector_json}); if (!el) throw new Error('scroll selector not found: ' + {selector_json}); el.scrollBy(0, {delta}); }})()"
            );
            let response = self
                .send_command("execute_js", json!({ "script": script }))
                .await?;
            Self::require_result(response, "execute_js")?;
            return Ok(());
        }
        self.expect_unit(
            "scroll",
            json!({
                "direction": direction,
                "pixels": pixels,
            }),
        )
        .await
    }

    async fn page_map(
        &mut self,
        scope: Option<&str>,
        compound_enrichment: bool,
        depth: Option<usize>,
    ) -> Result<Value, BridgeError> {
        let payload = build_page_map_payload(scope, compound_enrichment, depth);
        let response = self.send_command("page_map", payload).await?;
        Self::require_result(response, "page_map")
    }

    async fn extract_dom_snapshot(&mut self, scope: Option<&str>) -> Result<Value, BridgeError> {
        let mut payload = json!({});
        if let Some(s) = scope {
            payload["scope"] = json!(s);
        }
        let response = self.send_command("extract_dom_snapshot", payload).await?;
        Self::require_result(response, "extract_dom_snapshot")
    }

    async fn read_content(
        &mut self,
        heading: Option<&str>,
        selector: Option<&str>,
        offset: usize,
        max_chars: usize,
    ) -> Result<Value, BridgeError> {
        let response = self
            .send_command(
                "read_content",
                json!({
                    "heading": heading,
                    "selector": selector,
                    "offset": offset,
                    "max_chars": max_chars,
                }),
            )
            .await?;
        Self::require_result(response, "read_content")
    }

    async fn wait_for_selector(
        &mut self,
        selector: &str,
        timeout_ms: u64,
        state: Option<&str>,
    ) -> Result<bool, BridgeError> {
        let mut payload = json!({
            "selector": selector,
            "timeout_ms": timeout_ms,
        });
        if let Some(s) = state {
            payload["state"] = json!(s);
        }
        let response = self.send_command("wait_for_selector", payload).await?;
        let result = Self::require_result(response, "wait_for_selector")?;
        Ok(result
            .get("found")
            .and_then(Value::as_bool)
            .unwrap_or(false))
    }

    async fn select_option(&mut self, selector: &str, value: &str) -> Result<(), BridgeError> {
        self.expect_unit(
            "select_option",
            json!({
                "selector": selector,
                "value": value,
            }),
        )
        .await
    }

    async fn evaluate(&mut self, script: &str) -> Result<Value, BridgeError> {
        let response = self
            .send_command("execute_js", json!({ "script": script }))
            .await?;
        Self::require_result(response, "execute_js")
    }

    async fn hover(&mut self, selector: &str) -> Result<(), BridgeError> {
        self.expect_unit("hover", json!({ "selector": selector }))
            .await
    }

    async fn press_key(&mut self, key: &str, selector: Option<&str>) -> Result<(), BridgeError> {
        let mut payload = json!({ "key": key });
        if let Some(selector) = selector {
            payload["selector"] = Value::String(selector.to_string());
        }
        self.expect_unit("press_key", payload).await
    }

    async fn switch_tab(&mut self, index: i64) -> Result<Value, BridgeError> {
        let response = self
            .send_command("switch_tab", json!({ "index": index }))
            .await?;
        Self::require_result(response, "switch_tab")
    }

    async fn export_cookies(&mut self) -> Result<BrowserState, BridgeError> {
        let response = self.send_command("export_cookies", json!({})).await?;
        let result = Self::require_result(response, "export_cookies")?;
        Self::parse_result(result, "export_cookies")
    }

    async fn import_cookies(&mut self, state: &BrowserState) -> Result<(), BridgeError> {
        self.expect_unit(
            "import_cookies",
            json!({ "state": serde_json::to_value(state)? }),
        )
        .await
    }

    async fn import_cookies_only(&mut self, state: &BrowserState) -> Result<(), BridgeError> {
        self.expect_unit(
            "import_cookies_only",
            json!({ "state": serde_json::to_value(state)? }),
        )
        .await
    }

    async fn import_local_storage(&mut self, state: &BrowserState) -> Result<(), BridgeError> {
        self.expect_unit(
            "import_local_storage",
            json!({ "state": serde_json::to_value(state)? }),
        )
        .await
    }

    async fn list_resources(&mut self) -> Result<Value, BridgeError> {
        let response = self.send_command("list_resources", json!({})).await?;
        Self::require_result(response, "list_resources")
    }

    async fn save_file(
        &mut self,
        url: &str,
        path: &str,
        headers: Option<&BTreeMap<String, String>>,
    ) -> Result<String, BridgeError> {
        if path.contains("..") {
            return Err(BridgeError::Protocol(
                "save_file path contains path traversal".into(),
            ));
        }

        let headers = headers
            .map(serde_json::to_value)
            .transpose()?
            .unwrap_or_else(|| json!({}));

        let response = self
            .send_command(
                "save_file",
                json!({
                    "url": url,
                    "path": path,
                    "headers": headers,
                }),
            )
            .await?;
        let result = Self::require_result(response, "save_file")?;

        let data_base64 = result
            .get("data_base64")
            .and_then(Value::as_str)
            .ok_or_else(|| BridgeError::Protocol("save_file missing data_base64".into()))?;

        let bytes = base64::engine::general_purpose::STANDARD
            .decode(data_base64)
            .map_err(|e| BridgeError::Protocol(format!("base64 decode failed: {e}")))?;

        tokio::fs::write(path, &bytes)
            .await
            .map_err(|e| BridgeError::Protocol(format!("write failed: {e}")))?;

        Ok(path.to_string())
    }

    async fn click(&mut self, selector: &str) -> Result<(), BridgeError> {
        self.expect_unit("click", json!({ "selector": selector }))
            .await
    }

    async fn click_at(&mut self, x: f64, y: f64) -> Result<(), BridgeError> {
        self.expect_unit("click_at", json!({ "x": x, "y": y }))
            .await
    }

    async fn fill(&mut self, selector: &str, value: &str) -> Result<(), BridgeError> {
        self.expect_unit(
            "fill",
            json!({
                "selector": selector,
                "value": value,
            }),
        )
        .await
    }

    async fn screenshot(
        &mut self,
        options: &crate::ScreenshotOptions<'_>,
    ) -> Result<(String, usize), BridgeError> {
        let mut payload = json!({});
        if let Some(sel) = options.selector {
            payload["selector"] = json!(sel);
        }
        if let Some(fmt) = options.format {
            payload["format"] = json!(fmt);
        }
        if let Some(q) = options.quality {
            payload["quality"] = json!(q);
        }
        if options.full_page {
            payload["fullPage"] = json!(true);
        }
        let response = self.send_command("screenshot", payload).await?;
        let result = Self::require_result(response, "screenshot")?;
        let screenshot_base64 = result
            .get("screenshot_base64")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                BridgeError::Protocol("screenshot missing screenshot_base64".to_string())
            })?
            .to_string();
        let size_bytes = result
            .get("size_bytes")
            .and_then(Value::as_u64)
            .and_then(|value| usize::try_from(value).ok())
            .unwrap_or(0);
        Ok((screenshot_base64, size_bytes))
    }

    async fn go_back(&mut self) -> Result<String, BridgeError> {
        let response = self.send_command("go_back", json!({})).await?;
        let result = Self::require_result(response, "go_back")?;
        result
            .get("url")
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .ok_or_else(|| BridgeError::Protocol("go_back missing url".to_string()))
    }

    async fn reload(&mut self) -> Result<PageInfo, BridgeError> {
        let response = self.send_command("reload", json!({})).await?;
        let result = Self::require_result(response, "reload")?;
        let title = result
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let html = result
            .get("html")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        Ok(PageInfo {
            title,
            html: Some(html),
        })
    }

    async fn set_device(&mut self, options: &Value) -> Result<Value, BridgeError> {
        let response = self.send_command("set_device", options.clone()).await?;
        Self::require_result(response, "set_device")
    }

    async fn get_cookies(&mut self) -> Result<Vec<crate::CookieInfo>, BridgeError> {
        let response = self.send_command("get_cookies", json!({})).await?;
        let result = Self::require_result(response, "get_cookies")?;
        let cookies = result
            .get("cookies")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        cookies
            .into_iter()
            .map(|cookie| {
                serde_json::from_value::<crate::CookieInfo>(cookie)
                    .map_err(|e| BridgeError::Protocol(format!("failed to parse cookie: {e}")))
            })
            .collect()
    }

    async fn get_storage(
        &mut self,
        storage_type: crate::StorageType,
    ) -> Result<(Vec<crate::StorageEntry>, Vec<crate::StorageEntry>), BridgeError> {
        let storage_type_str = match storage_type {
            crate::StorageType::Local => "local",
            crate::StorageType::Session => "session",
            crate::StorageType::All => "all",
        };
        let response = self
            .send_command("get_storage", json!({ "storage_type": storage_type_str }))
            .await?;
        let result = Self::require_result(response, "get_storage")?;

        let local_storage = result
            .get("local_storage")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let session_storage = result
            .get("session_storage")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        let local_entries: Result<Vec<_>, _> = local_storage
            .into_iter()
            .map(|entry| {
                serde_json::from_value::<crate::StorageEntry>(entry).map_err(|e| {
                    BridgeError::Protocol(format!("failed to parse storage entry: {e}"))
                })
            })
            .collect();

        let session_entries: Result<Vec<_>, _> = session_storage
            .into_iter()
            .map(|entry| {
                serde_json::from_value::<crate::StorageEntry>(entry).map_err(|e| {
                    BridgeError::Protocol(format!("failed to parse storage entry: {e}"))
                })
            })
            .collect();

        Ok((local_entries?, session_entries?))
    }

    async fn start_coverage(&mut self, js: bool, css: bool) -> Result<(), BridgeError> {
        let response = self
            .send_command("start_coverage", json!({ "js": js, "css": css }))
            .await?;
        Self::require_ok(response)?;
        Ok(())
    }

    async fn stop_coverage(&mut self) -> Result<crate::CoverageData, BridgeError> {
        let response = self.send_command("stop_coverage", json!({})).await?;
        let result = Self::require_result(response, "stop_coverage")?;

        let js_entries = result
            .get("js_coverage")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let css_entries = result
            .get("css_coverage")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        let js_coverage = js_entries
            .into_iter()
            .filter_map(|entry| {
                Some(crate::FileCoverage {
                    url: entry.get("url")?.as_str()?.to_string(),
                    total_bytes: usize::try_from(entry.get("total_bytes")?.as_u64()?).ok()?,
                    used_bytes: usize::try_from(entry.get("used_bytes")?.as_u64()?).ok()?,
                })
            })
            .collect();

        let css_coverage = css_entries
            .into_iter()
            .filter_map(|entry| {
                Some(crate::FileCoverage {
                    url: entry.get("url")?.as_str()?.to_string(),
                    total_bytes: usize::try_from(entry.get("total_bytes")?.as_u64()?).ok()?,
                    used_bytes: usize::try_from(entry.get("used_bytes")?.as_u64()?).ok()?,
                })
            })
            .collect();

        Ok(crate::CoverageData {
            js_coverage,
            css_coverage,
        })
    }

    async fn add_intercept_rule(
        &mut self,
        rule: crate::InterceptRule,
    ) -> Result<String, BridgeError> {
        use std::time::{SystemTime, UNIX_EPOCH};
        let rule_id = format!(
            "rule_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .subsec_nanos()
        );
        self.send_command(
            "add_intercept_rule",
            serde_json::json!({
                "rule_id": rule_id,
                "pattern": rule.pattern,
                "action_type": format!("{:?}", rule.action),
                "mock": rule.mock,
            }),
        )
        .await?;
        Ok(rule_id)
    }

    async fn remove_intercept_rule(&mut self, rule_id: &str) -> Result<(), BridgeError> {
        self.send_command(
            "remove_intercept_rule",
            serde_json::json!({"rule_id": rule_id}),
        )
        .await?;
        Ok(())
    }

    async fn clear_intercept_rules(&mut self) -> Result<(), BridgeError> {
        self.send_command("clear_intercept_rules", serde_json::json!({}))
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::mem;

    use super::*;

    fn bridge() -> (
        ExtensionBridge,
        mpsc::Receiver<(BridgeCommand, oneshot::Sender<BridgeResponse>)>,
    ) {
        let (command_tx, command_rx) = mpsc::channel(1);
        let (_tx, connected) = watch::channel(true);
        (ExtensionBridge::new(command_tx, connected), command_rx)
    }

    #[test]
    fn page_map_payload_forwards_scope_enrichment_and_depth() {
        assert_eq!(build_page_map_payload(None, false, None), json!({}));
        assert_eq!(
            build_page_map_payload(Some("main"), true, Some(7)),
            json!({ "scope": "main", "compoundEnrichment": true, "depth": 7 })
        );
        assert_eq!(
            build_page_map_payload(None, false, Some(10)),
            json!({ "depth": 10 })
        );
    }

    #[tokio::test]
    async fn navigate_serializes_expected_bridge_command() {
        let (mut bridge, mut command_rx) = bridge();

        let task = tokio::spawn(async move { bridge.navigate("https://example.com").await });

        let (command, resp_tx) = command_rx.recv().await.expect("command should be sent");
        assert_eq!(command.id, 1);
        assert_eq!(command.action, "navigate");
        assert_eq!(command.payload, json!({ "url": "https://example.com" }));

        resp_tx
            .send(BridgeResponse {
                id: command.id,
                ok: true,
                result: Some(json!({
                    "title": "Example Domain",
                    "html": "<html></html>",
                })),
                error: None,
            })
            .expect("response should be delivered");

        let page = task
            .await
            .expect("task should complete")
            .expect("navigate should succeed");
        assert_eq!(
            page,
            PageInfo {
                title: "Example Domain".to_string(),
                html: Some("<html></html>".to_string()),
            }
        );
    }

    #[tokio::test]
    async fn import_cookies_serializes_state_payload() {
        let (mut bridge, mut command_rx) = bridge();
        let state = BrowserState {
            cookies: json!([{ "name": "sid", "value": "abc" }]),
            local_storage: json!({ "token": "xyz" }),
            url: "https://example.com".to_string(),
        };

        let task = tokio::spawn(async move { bridge.import_cookies(&state).await });

        let (command, resp_tx) = command_rx.recv().await.expect("command should be sent");
        assert_eq!(command.action, "import_cookies");
        assert_eq!(
            command.payload,
            json!({
                "state": {
                    "cookies": [{ "name": "sid", "value": "abc" }],
                    "local_storage": { "token": "xyz" },
                    "url": "https://example.com",
                }
            })
        );

        resp_tx
            .send(BridgeResponse {
                id: command.id,
                ok: true,
                result: Some(json!({ "imported": true })),
                error: None,
            })
            .expect("response should be delivered");

        task.await
            .expect("task should complete")
            .expect("import_cookies should succeed");
    }

    #[tokio::test]
    async fn disconnect_error_propagates_as_extension_disconnected() {
        let (mut bridge, command_rx) = bridge();
        drop(command_rx);

        let error = bridge.click("#submit").await.expect_err("send should fail");
        assert!(matches!(error, BridgeError::ExtensionDisconnected));
    }

    #[tokio::test]
    async fn timeout_returns_extension_timeout() {
        let (bridge, mut command_rx) = bridge();

        let task = tokio::spawn(async move {
            bridge
                .send_command_with_timeout("page_map", json!({}), EXTENSION_COMMAND_TIMEOUT)
                .await
        });

        let (_command, resp_tx) = command_rx.recv().await.expect("command should be sent");
        mem::forget(resp_tx);

        let error = task
            .await
            .expect("task should complete")
            .expect_err("timeout should error");
        assert!(matches!(
            error,
            BridgeError::ExtensionTimeout { timeout } if timeout == EXTENSION_COMMAND_TIMEOUT
        ));
    }

    #[tokio::test]
    async fn fails_fast_when_disconnected() {
        let (tx, _rx) = mpsc::channel(10);
        let (_wtx, connected) = watch::channel(false);
        let mut bridge = ExtensionBridge::new(tx, connected);

        let result = bridge.navigate("https://example.com").await;
        assert!(matches!(result, Err(BridgeError::ExtensionDisconnected)));
    }

    #[tokio::test]
    async fn timeout_when_no_response() {
        let (tx, mut rx) = mpsc::channel(10);
        let (_wtx, connected) = watch::channel(true);
        let mut bridge = ExtensionBridge::new(tx, connected);

        tokio::spawn(async move {
            let _ = rx.recv().await;
        });

        let result = bridge.navigate("https://example.com").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn successful_navigate() {
        let (tx, mut rx) = mpsc::channel(10);
        let (_wtx, connected) = watch::channel(true);
        let mut bridge = ExtensionBridge::new(tx, connected);

        tokio::spawn(async move {
            if let Some((cmd, reply)) = rx.recv().await {
                let _ = reply.send(BridgeResponse {
                    id: cmd.id,
                    ok: true,
                    result: Some(serde_json::json!({
                        "title": "Example",
                        "html": "<html></html>"
                    })),
                    error: None,
                });
            }
        });

        let result = bridge.navigate("https://example.com").await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().title, "Example");
    }

    #[tokio::test]
    async fn error_response_maps_to_bridge_error() {
        let (tx, mut rx) = mpsc::channel(10);
        let (_wtx, connected) = watch::channel(true);
        let mut bridge = ExtensionBridge::new(tx, connected);

        tokio::spawn(async move {
            if let Some((cmd, reply)) = rx.recv().await {
                let _ = reply.send(BridgeResponse {
                    id: cmd.id,
                    ok: false,
                    result: None,
                    error: Some("element not found".to_string()),
                });
            }
        });

        let result = bridge.click("#missing").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn channel_closed_returns_error() {
        let (tx, rx) = mpsc::channel(10);
        let (_wtx, connected) = watch::channel(true);
        let mut bridge = ExtensionBridge::new(tx, connected);
        drop(rx);

        let result = bridge.navigate("https://example.com").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn poll_observations_parses_supported_event_types() {
        let (mut bridge, mut command_rx) = bridge();

        let task = tokio::spawn(async move { bridge.poll_observations().await });

        let (command, resp_tx) = command_rx.recv().await.expect("command should be sent");
        assert_eq!(command.action, "poll_observations");
        assert_eq!(command.payload, json!({}));

        resp_tx
            .send(BridgeResponse {
                id: command.id,
                ok: true,
                result: Some(json!({
                    "events": [
                        {
                            "type": "NetworkRequest",
                            "timestamp_ms": 1,
                            "tab_index": 0,
                            "seq_at_initiation": 7,
                            "request_id": "req-1",
                            "url": "https://example.com/api",
                            "method": "GET",
                            "status": 200,
                            "state": "Completed",
                            "size_bytes": 42,
                            "duration_ms": 12,
                            "request_type": "Fetch",
                            "from_service_worker": false,
                            "initiator_type": "script",
                            "reason": null
                        },
                        {
                            "type": "ConsoleMessage",
                            "timestamp_ms": 2,
                            "tab_index": 0,
                            "seq_at_initiation": 7,
                            "level": "log",
                            "message_type": "Console",
                            "text": "hello",
                            "source_url": null,
                            "source_line": null,
                            "source_column": null,
                            "stack": null
                        },
                        {
                            "type": "WebSocketFrame",
                            "timestamp_ms": 3,
                            "tab_index": 0,
                            "seq_at_initiation": 7,
                            "connection_id": "socket-1",
                            "url": "wss://example.com/socket",
                            "direction": "received",
                            "data": "payload",
                            "size_bytes": 7,
                            "connection_status": "open"
                        }
                    ]
                })),
                error: None,
            })
            .expect("response should be delivered");

        let events = task
            .await
            .expect("task should complete")
            .expect("poll_observations should succeed");

        assert_eq!(events.len(), 3);
        assert!(matches!(events[0], ObservationEvent::NetworkRequest(_)));
        assert!(matches!(events[1], ObservationEvent::ConsoleMessage(_)));
        assert!(matches!(events[2], ObservationEvent::WebSocketFrame(_)));
    }

    #[tokio::test]
    async fn set_seq_serializes_expected_bridge_command() {
        let (mut bridge, mut command_rx) = bridge();

        let task = tokio::spawn(async move { bridge.set_seq(99).await });

        let (command, resp_tx) = command_rx.recv().await.expect("command should be sent");
        assert_eq!(command.action, "set_seq");
        assert_eq!(command.payload, json!({ "seq": 99 }));

        resp_tx
            .send(BridgeResponse {
                id: command.id,
                ok: true,
                result: Some(json!({})),
                error: None,
            })
            .expect("response should be delivered");

        task.await
            .expect("task should complete")
            .expect("set_seq should succeed");
    }
}
