use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot};

use crate::ws_server::{BridgeCommand, BridgeResponse};
use crate::{BrowserBackend, BrowserState, PageInfo, PlaywrightBridgeError};

#[cfg(not(test))]
const EXTENSION_COMMAND_TIMEOUT: Duration = Duration::from_secs(60);
#[cfg(test)]
const EXTENSION_COMMAND_TIMEOUT: Duration = Duration::from_millis(50);

pub struct ExtensionBridge {
    command_tx: mpsc::Sender<(BridgeCommand, oneshot::Sender<BridgeResponse>)>,
    next_id: Arc<AtomicU64>,
}

impl ExtensionBridge {
    #[must_use]
    pub fn new(command_tx: mpsc::Sender<(BridgeCommand, oneshot::Sender<BridgeResponse>)>) -> Self {
        Self {
            command_tx,
            next_id: Arc::new(AtomicU64::new(1)),
        }
    }

    async fn send_command(
        &self,
        action: &str,
        payload: Value,
    ) -> Result<BridgeResponse, PlaywrightBridgeError> {
        self.send_command_with_timeout(action, payload, EXTENSION_COMMAND_TIMEOUT)
            .await
    }

    async fn send_command_with_timeout(
        &self,
        action: &str,
        payload: Value,
        timeout: Duration,
    ) -> Result<BridgeResponse, PlaywrightBridgeError> {
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
            .map_err(|_| PlaywrightBridgeError::ExtensionDisconnected)?;

        tokio::time::timeout(timeout, resp_rx)
            .await
            .map_err(|_| PlaywrightBridgeError::ExtensionTimeout { timeout })?
            .map_err(|_| PlaywrightBridgeError::ExtensionDisconnected)
    }

    fn require_ok(response: BridgeResponse) -> Result<BridgeResponse, PlaywrightBridgeError> {
        if response.ok {
            Ok(response)
        } else {
            Err(PlaywrightBridgeError::Protocol(
                response.error.unwrap_or_default(),
            ))
        }
    }

    fn require_result(
        response: BridgeResponse,
        action: &str,
    ) -> Result<Value, PlaywrightBridgeError> {
        Self::require_ok(response)?
            .result
            .ok_or_else(|| PlaywrightBridgeError::Protocol(format!("{action} missing result")))
    }

    fn parse_result<T: DeserializeOwned>(
        result: Value,
        action: &str,
    ) -> Result<T, PlaywrightBridgeError> {
        serde_json::from_value(result).map_err(|error| {
            PlaywrightBridgeError::Protocol(format!("{action} result parse error: {error}"))
        })
    }

    async fn expect_unit(&self, action: &str, payload: Value) -> Result<(), PlaywrightBridgeError> {
        let response = self.send_command(action, payload).await?;
        Self::require_ok(response)?;
        Ok(())
    }
}

impl fmt::Debug for ExtensionBridge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExtensionBridge").finish_non_exhaustive()
    }
}

#[async_trait]
impl BrowserBackend for ExtensionBridge {
    async fn navigate(&mut self, url: &str) -> Result<PageInfo, PlaywrightBridgeError> {
        let response = self.send_command("navigate", json!({ "url": url })).await?;
        let result = Self::require_result(response, "navigate")?;
        Self::parse_result(result, "navigate")
    }

    async fn new_page(&mut self, url: Option<&str>) -> Result<usize, PlaywrightBridgeError> {
        let payload = url.map_or_else(|| json!({}), |url| json!({ "url": url }));
        let response = self.send_command("new_page", payload).await?;
        let result = Self::require_result(response, "new_page")?;
        let page_index = result
            .get("pageIndex")
            .and_then(Value::as_u64)
            .ok_or_else(|| {
                PlaywrightBridgeError::Protocol("new_page missing pageIndex".to_string())
            })?;
        usize::try_from(page_index).map_err(|_| {
            PlaywrightBridgeError::Protocol(format!(
                "new_page returned out-of-range pageIndex {page_index}"
            ))
        })
    }

    async fn close_page(&mut self, page_index: usize) -> Result<(), PlaywrightBridgeError> {
        self.expect_unit("close_page", json!({ "page_index": page_index }))
            .await
    }

    async fn scroll(&mut self, direction: &str, pixels: i64) -> Result<(), PlaywrightBridgeError> {
        self.expect_unit(
            "scroll",
            json!({
                "direction": direction,
                "pixels": pixels,
            }),
        )
        .await
    }

    async fn page_map(&mut self) -> Result<Value, PlaywrightBridgeError> {
        let response = self.send_command("page_map", json!({})).await?;
        Self::require_result(response, "page_map")
    }

    async fn read_content(
        &mut self,
        heading: Option<&str>,
        selector: Option<&str>,
        offset: usize,
        max_chars: usize,
    ) -> Result<Value, PlaywrightBridgeError> {
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
    ) -> Result<bool, PlaywrightBridgeError> {
        let response = self
            .send_command(
                "wait_for_selector",
                json!({
                    "selector": selector,
                    "timeout_ms": timeout_ms,
                }),
            )
            .await?;
        let result = Self::require_result(response, "wait_for_selector")?;
        Ok(result
            .get("found")
            .and_then(Value::as_bool)
            .unwrap_or(false))
    }

    async fn select_option(
        &mut self,
        selector: &str,
        value: &str,
    ) -> Result<(), PlaywrightBridgeError> {
        self.expect_unit(
            "select_option",
            json!({
                "selector": selector,
                "value": value,
            }),
        )
        .await
    }

    async fn evaluate(&mut self, script: &str) -> Result<Value, PlaywrightBridgeError> {
        let response = self
            .send_command("evaluate", json!({ "script": script }))
            .await?;
        Self::require_result(response, "evaluate")
    }

    async fn hover(&mut self, selector: &str) -> Result<(), PlaywrightBridgeError> {
        self.expect_unit("hover", json!({ "selector": selector }))
            .await
    }

    async fn press_key(
        &mut self,
        key: &str,
        selector: Option<&str>,
    ) -> Result<(), PlaywrightBridgeError> {
        let mut payload = json!({ "key": key });
        if let Some(selector) = selector {
            payload["selector"] = Value::String(selector.to_string());
        }
        self.expect_unit("press_key", payload).await
    }

    async fn switch_tab(&mut self, index: i64) -> Result<Value, PlaywrightBridgeError> {
        let response = self
            .send_command("switch_tab", json!({ "index": index }))
            .await?;
        Self::require_result(response, "switch_tab")
    }

    async fn export_cookies(&mut self) -> Result<BrowserState, PlaywrightBridgeError> {
        let response = self.send_command("export_cookies", json!({})).await?;
        let result = Self::require_result(response, "export_cookies")?;
        Self::parse_result(result, "export_cookies")
    }

    async fn import_cookies(&mut self, state: &BrowserState) -> Result<(), PlaywrightBridgeError> {
        self.expect_unit(
            "import_cookies",
            json!({ "state": serde_json::to_value(state)? }),
        )
        .await
    }

    async fn import_cookies_only(
        &mut self,
        state: &BrowserState,
    ) -> Result<(), PlaywrightBridgeError> {
        self.expect_unit(
            "import_cookies_only",
            json!({ "state": serde_json::to_value(state)? }),
        )
        .await
    }

    async fn import_local_storage(
        &mut self,
        state: &BrowserState,
    ) -> Result<(), PlaywrightBridgeError> {
        self.expect_unit(
            "import_local_storage",
            json!({ "state": serde_json::to_value(state)? }),
        )
        .await
    }

    async fn list_resources(&mut self) -> Result<Value, PlaywrightBridgeError> {
        let response = self.send_command("list_resources", json!({})).await?;
        Self::require_result(response, "list_resources")
    }

    async fn save_file(&mut self, url: &str, path: &str) -> Result<String, PlaywrightBridgeError> {
        let response = self
            .send_command(
                "save_file",
                json!({
                    "url": url,
                    "path": path,
                }),
            )
            .await?;
        let result = Self::require_result(response, "save_file")?;
        Ok(result
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or(path)
            .to_string())
    }

    async fn click(&mut self, selector: &str) -> Result<(), PlaywrightBridgeError> {
        self.expect_unit("click", json!({ "selector": selector }))
            .await
    }

    async fn fill(&mut self, selector: &str, value: &str) -> Result<(), PlaywrightBridgeError> {
        self.expect_unit(
            "fill",
            json!({
                "selector": selector,
                "value": value,
            }),
        )
        .await
    }

    async fn screenshot(&mut self) -> Result<(String, usize), PlaywrightBridgeError> {
        let response = self.send_command("screenshot", json!({})).await?;
        let result = Self::require_result(response, "screenshot")?;
        let screenshot_base64 = result
            .get("screenshot_base64")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                PlaywrightBridgeError::Protocol("screenshot missing screenshot_base64".to_string())
            })?
            .to_string();
        let size_bytes = result
            .get("size_bytes")
            .and_then(Value::as_u64)
            .and_then(|value| usize::try_from(value).ok())
            .unwrap_or(0);
        Ok((screenshot_base64, size_bytes))
    }

    async fn go_back(&mut self) -> Result<String, PlaywrightBridgeError> {
        let response = self.send_command("go_back", json!({})).await?;
        let result = Self::require_result(response, "go_back")?;
        result
            .get("url")
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .ok_or_else(|| PlaywrightBridgeError::Protocol("go_back missing url".to_string()))
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
        (ExtensionBridge::new(command_tx), command_rx)
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
                html: "<html></html>".to_string(),
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
        assert!(matches!(
            error,
            PlaywrightBridgeError::ExtensionDisconnected
        ));
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
            PlaywrightBridgeError::ExtensionTimeout { timeout } if timeout == EXTENSION_COMMAND_TIMEOUT
        ));
    }
}
