use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use acrawl_core::{child_stderr, config_home_dir};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;
use tokio::time::timeout;

use super::bridge_script::PLAYWRIGHT_BRIDGE_NODE_SCRIPT;
use super::types::{
    BridgeBootstrapMessage, BridgeCommandEnvelope, BridgeError, BridgeResponseMessage,
    BrowserState, GenericBridgeResponseMessage, PageInfo,
};
use super::{
    CLOSE_COMMAND_TIMEOUT, DEFAULT_COMMAND_TIMEOUT, DEFAULT_LAUNCH_TIMEOUT,
    DEFAULT_SHUTDOWN_TIMEOUT,
};
use crate::browser_backend::BrowserBackend;

#[derive(Debug)]
pub struct PlaywrightBridge {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

pub type SharedBridge = Arc<Mutex<Box<dyn BrowserBackend + Send>>>;

impl PlaywrightBridge {
    pub async fn new() -> Result<Self, BridgeError> {
        let args = if cfg!(windows) {
            let config_dir = config_home_dir();
            std::fs::create_dir_all(&config_dir)
                .map_err(|e| BridgeError::Protocol(format!("failed to create config dir: {e}")))?;
            let script_path = config_dir.join("bridge.cjs");
            std::fs::write(&script_path, PLAYWRIGHT_BRIDGE_NODE_SCRIPT).map_err(|e| {
                BridgeError::Protocol(format!("failed to write bridge script: {e}"))
            })?;
            vec![script_path.to_string_lossy().into_owned()]
        } else {
            vec!["-e".to_string(), PLAYWRIGHT_BRIDGE_NODE_SCRIPT.to_string()]
        };

        Self::new_with_invocation("node", args, DEFAULT_LAUNCH_TIMEOUT).await
    }

    async fn new_with_invocation(
        program: &str,
        args: Vec<String>,
        launch_timeout: Duration,
    ) -> Result<Self, BridgeError> {
        let mut command = Self::bridge_command(program, &args);

        let mut child = command
            .spawn()
            .map_err(|source| BridgeError::ProcessSpawn {
                command: format!("{program} {}", args.join(" ")),
                source,
            })?;

        let stdin = child.stdin.take().ok_or_else(|| {
            BridgeError::Protocol("bridge process missing stdin pipe".to_string())
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            BridgeError::Protocol("bridge process missing stdout pipe".to_string())
        })?;
        let mut bridge = Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
        };

        if let Ok(result) = timeout(launch_timeout, bridge.read_bootstrap_message()).await {
            result?;
        } else {
            bridge.force_kill_child().await?;
            return Err(BridgeError::LaunchTimeout {
                timeout: launch_timeout,
            });
        }

        Ok(bridge)
    }

    fn bridge_command(program: &str, args: &[String]) -> Command {
        let mut command = Command::new(program);
        let acrawl_node_modules = config_home_dir().join("node_modules");
        let mut paths: Vec<std::path::PathBuf> = match std::env::var_os("NODE_PATH") {
            Some(existing) => std::env::split_paths(&existing).collect(),
            None => Vec::new(),
        };
        if !paths.contains(&acrawl_node_modules) {
            paths.insert(0, acrawl_node_modules.clone());
        }
        if let Ok(cwd) = std::env::current_dir() {
            let cwd_node_modules = cwd.join("node_modules");
            if !paths.contains(&cwd_node_modules) {
                paths.push(cwd_node_modules);
            }
        }
        let node_path = std::env::join_paths(&paths).unwrap_or_else(|_| acrawl_node_modules.into());
        command
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(child_stderr())
            .kill_on_drop(true)
            .env("NODE_PATH", node_path);
        command
    }

    pub async fn navigate(&mut self, url: &str) -> Result<PageInfo, BridgeError> {
        let response = self
            .send_bridge_command(BridgeCommandEnvelope {
                action: "navigate",
                url: Some(url),
            })
            .await?;

        if response.ok {
            return response.result.ok_or_else(|| {
                BridgeError::Protocol("navigate response missing page result payload".to_string())
            });
        }

        let error = response.error.ok_or_else(|| {
            BridgeError::Protocol("navigate response missing error payload".to_string())
        })?;
        Err(BridgeError::Protocol(format!(
            "{}: {}",
            error.kind, error.message
        )))
    }

    pub async fn close(mut self) -> Result<(), BridgeError> {
        let _ = self
            .send_bridge_command_with_timeout(
                BridgeCommandEnvelope {
                    action: "close",
                    url: None,
                },
                CLOSE_COMMAND_TIMEOUT,
            )
            .await;

        if let Ok(wait_result) = timeout(DEFAULT_SHUTDOWN_TIMEOUT, self.child.wait()).await {
            let _status = wait_result?;
            Ok(())
        } else {
            self.force_kill_child().await?;
            Err(BridgeError::ShutdownTimeout {
                timeout: DEFAULT_SHUTDOWN_TIMEOUT,
            })
        }
    }

    pub async fn new_page(&mut self, url: Option<&str>) -> Result<usize, BridgeError> {
        let mut cmd = serde_json::json!({ "action": "new_page" });
        if let Some(url) = url {
            cmd["url"] = serde_json::Value::String(url.to_string());
        }
        let result = self.send_raw_command(&cmd).await?;
        let page_index = result
            .get("pageIndex")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| {
                BridgeError::Protocol("new_page response missing pageIndex".to_string())
            })?;
        usize::try_from(page_index).map_err(|_| {
            BridgeError::Protocol(format!(
                "new_page returned out-of-range pageIndex {page_index}"
            ))
        })
    }

    pub async fn close_page(&mut self, page_index: usize) -> Result<(), BridgeError> {
        let cmd = serde_json::json!({
            "action": "close_page",
            "pageIndex": page_index,
        });
        self.send_raw_command(&cmd).await?;
        Ok(())
    }

    pub async fn send_raw_command(
        &mut self,
        command: &serde_json::Value,
    ) -> Result<serde_json::Value, BridgeError> {
        let payload = serde_json::to_string(command)?;
        self.stdin.write_all(payload.as_bytes()).await?;
        self.stdin.write_all(b"\n").await?;
        self.stdin.flush().await?;

        let line = timeout(DEFAULT_COMMAND_TIMEOUT, self.read_bridge_line())
            .await
            .map_err(|_| BridgeError::CommandTimeout {
                timeout: DEFAULT_COMMAND_TIMEOUT,
            })??;
        let response: GenericBridgeResponseMessage = serde_json::from_str(&line)?;
        if response.event != "bridge_response" {
            return Err(BridgeError::Protocol(format!(
                "expected bridge_response event, got {}",
                response.event
            )));
        }
        if response.ok {
            return Ok(response.result.unwrap_or(serde_json::Value::Null));
        }
        let error = response
            .error
            .ok_or_else(|| BridgeError::Protocol("response missing error payload".to_string()))?;
        Err(BridgeError::Protocol(format!(
            "{}: {}",
            error.kind, error.message
        )))
    }

    pub async fn scroll(&mut self, direction: &str, pixels: i64) -> Result<(), BridgeError> {
        let cmd = serde_json::json!({
            "action": "scroll",
            "direction": direction,
            "pixels": pixels,
        });
        self.send_raw_command(&cmd).await?;
        Ok(())
    }

    pub async fn page_map(
        &mut self,
        scope: Option<&str>,
        compound_enrichment: bool,
    ) -> Result<serde_json::Value, BridgeError> {
        let mut cmd = serde_json::json!({ "action": "page_map" });
        if let Some(s) = scope {
            cmd["scope"] = serde_json::Value::String(s.to_string());
        }
        if compound_enrichment {
            cmd["compoundEnrichment"] = serde_json::Value::Bool(true);
        }
        self.send_raw_command(&cmd).await
    }

    pub async fn read_content(
        &mut self,
        heading: Option<&str>,
        selector: Option<&str>,
        offset: usize,
        max_chars: usize,
    ) -> Result<serde_json::Value, BridgeError> {
        let cmd = serde_json::json!({
            "action": "read_content",
            "heading": heading,
            "selector": selector,
            "offset": offset,
            "max_chars": max_chars,
        });
        self.send_raw_command(&cmd).await
    }

    pub async fn wait_for_selector(
        &mut self,
        selector: &str,
        timeout_ms: u64,
        state: Option<&str>,
    ) -> Result<bool, BridgeError> {
        let mut cmd = serde_json::json!({
            "action": "wait_for_selector",
            "selector": selector,
            "timeout_ms": timeout_ms,
        });
        if let Some(s) = state {
            cmd["state"] = serde_json::Value::String(s.to_string());
        }
        let result = self.send_raw_command(&cmd).await?;
        Ok(result
            .get("found")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false))
    }

    pub async fn select_option(&mut self, selector: &str, value: &str) -> Result<(), BridgeError> {
        let cmd = serde_json::json!({
            "action": "select_option",
            "selector": selector,
            "value": value,
        });
        self.send_raw_command(&cmd).await?;
        Ok(())
    }

    pub async fn evaluate(&mut self, script: &str) -> Result<serde_json::Value, BridgeError> {
        let cmd = serde_json::json!({
            "action": "evaluate",
            "script": script,
        });
        self.send_raw_command(&cmd).await
    }

    pub async fn hover(&mut self, selector: &str) -> Result<(), BridgeError> {
        let cmd = serde_json::json!({
            "action": "hover",
            "selector": selector,
        });
        self.send_raw_command(&cmd).await?;
        Ok(())
    }

    pub async fn press_key(
        &mut self,
        key: &str,
        selector: Option<&str>,
    ) -> Result<(), BridgeError> {
        let mut cmd = serde_json::json!({
            "action": "press_key",
            "key": key,
        });
        if let Some(sel) = selector {
            cmd["selector"] = serde_json::Value::String(sel.to_string());
        }
        self.send_raw_command(&cmd).await?;
        Ok(())
    }

    pub async fn switch_tab(&mut self, index: i64) -> Result<serde_json::Value, BridgeError> {
        let cmd = serde_json::json!({
            "action": "switch_tab",
            "index": index,
        });
        self.send_raw_command(&cmd).await
    }

    pub async fn export_cookies(&mut self) -> Result<BrowserState, BridgeError> {
        let cmd = serde_json::json!({ "action": "export_cookies" });
        let result = self.send_raw_command(&cmd).await?;
        let state = serde_json::from_value::<BrowserState>(result)
            .map_err(|e| BridgeError::Protocol(format!("failed to parse BrowserState: {e}")))?;
        Ok(state)
    }

    pub async fn import_cookies(&mut self, state: &BrowserState) -> Result<(), BridgeError> {
        let cmd = serde_json::json!({
            "action": "import_cookies",
            "cookies": state.cookies,
            "local_storage": state.local_storage,
        });
        self.send_raw_command(&cmd).await?;
        Ok(())
    }

    pub async fn import_cookies_only(&mut self, state: &BrowserState) -> Result<(), BridgeError> {
        let cmd = serde_json::json!({
            "action": "import_cookies",
            "cookies": state.cookies,
        });
        self.send_raw_command(&cmd).await?;
        Ok(())
    }

    pub async fn import_local_storage(&mut self, state: &BrowserState) -> Result<(), BridgeError> {
        let cmd = serde_json::json!({
            "action": "import_cookies",
            "local_storage": state.local_storage,
        });
        self.send_raw_command(&cmd).await?;
        Ok(())
    }

    pub async fn set_device(
        &mut self,
        options: &serde_json::Value,
    ) -> Result<serde_json::Value, BridgeError> {
        let mut cmd = serde_json::json!({ "action": "set_device" });
        if let serde_json::Value::Object(map) = options {
            for (k, v) in map {
                cmd[k] = v.clone();
            }
        }
        self.send_raw_command(&cmd).await
    }

    pub async fn poll_observations_raw(
        &mut self,
        tab_index: usize,
    ) -> Result<Vec<serde_json::Value>, BridgeError> {
        let result = self
            .send_raw_command(&serde_json::json!({
                "action": "poll_observations",
                "tab_index": tab_index,
            }))
            .await?;
        Ok(result
            .get("events")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default())
    }

    pub async fn set_seq_raw(&mut self, seq: u64) -> Result<(), BridgeError> {
        self.send_raw_command(&serde_json::json!({
            "action": "set_seq",
            "seq": seq,
        }))
        .await?;
        Ok(())
    }

    pub async fn list_resources(&mut self) -> Result<serde_json::Value, BridgeError> {
        let cmd = serde_json::json!({ "action": "list_resources" });
        self.send_raw_command(&cmd).await
    }

    pub async fn save_file(&mut self, url: &str, path: &str) -> Result<String, BridgeError> {
        let cmd = serde_json::json!({
            "action": "save_file",
            "url": url,
            "path": path,
        });
        let result = self.send_raw_command(&cmd).await?;
        Ok(result
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or(path)
            .to_string())
    }

    pub async fn click(&mut self, selector: &str) -> Result<(), BridgeError> {
        let cmd = serde_json::json!({
            "action": "click",
            "selector": selector,
        });
        self.send_raw_command(&cmd).await?;
        Ok(())
    }

    pub async fn click_at(&mut self, x: f64, y: f64) -> Result<(), BridgeError> {
        let cmd = serde_json::json!({
            "action": "click_at",
            "x": x,
            "y": y,
        });
        self.send_raw_command(&cmd).await?;
        Ok(())
    }

    pub async fn fill(&mut self, selector: &str, value: &str) -> Result<(), BridgeError> {
        let cmd = serde_json::json!({
            "action": "fill",
            "selector": selector,
            "value": value,
        });
        self.send_raw_command(&cmd).await?;
        Ok(())
    }

    pub async fn screenshot(
        &mut self,
        options: &crate::ScreenshotOptions<'_>,
    ) -> Result<(String, usize), BridgeError> {
        let mut cmd = serde_json::json!({ "action": "screenshot" });
        if let Some(sel) = options.selector {
            cmd["selector"] = serde_json::Value::String(sel.to_string());
        }
        if let Some(fmt) = options.format {
            cmd["format"] = serde_json::Value::String(fmt.to_string());
        }
        if let Some(q) = options.quality {
            cmd["quality"] = serde_json::json!(q);
        }
        if options.full_page {
            cmd["fullPage"] = serde_json::json!(true);
        }
        let result = self.send_raw_command(&cmd).await?;
        let base64_data = result
            .get("screenshot_base64")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| {
                BridgeError::Protocol("screenshot response missing base64 data".to_string())
            })?
            .to_string();
        #[allow(clippy::cast_possible_truncation)]
        let size_bytes = result
            .get("size_bytes")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0) as usize;
        Ok((base64_data, size_bytes))
    }

    pub async fn go_back(&mut self) -> Result<String, BridgeError> {
        let cmd = serde_json::json!({ "action": "go_back" });
        let result = self.send_raw_command(&cmd).await?;
        let url = result
            .get("url")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_string();
        Ok(url)
    }

    async fn read_bootstrap_message(&mut self) -> Result<(), BridgeError> {
        let line = self.read_bridge_line().await?;
        let message: BridgeBootstrapMessage = serde_json::from_str(&line)?;

        if message.event != "bridge_bootstrap" {
            return Err(BridgeError::Protocol(format!(
                "expected bridge_bootstrap event, got {}",
                message.event
            )));
        }

        if message.ok {
            return Ok(());
        }

        let error = message.error.ok_or_else(|| {
            BridgeError::Protocol("bootstrap failure missing error payload".to_string())
        })?;

        if error.kind == "playwright_not_installed" {
            return Err(BridgeError::PlaywrightNotInstalled(error.message));
        }

        Err(BridgeError::Protocol(format!(
            "bootstrap failed: {} ({})",
            error.message, error.kind
        )))
    }

    async fn send_bridge_command(
        &mut self,
        command: BridgeCommandEnvelope<'_>,
    ) -> Result<BridgeResponseMessage, BridgeError> {
        self.send_bridge_command_with_timeout(command, DEFAULT_COMMAND_TIMEOUT)
            .await
    }

    async fn send_bridge_command_with_timeout(
        &mut self,
        command: BridgeCommandEnvelope<'_>,
        command_timeout: Duration,
    ) -> Result<BridgeResponseMessage, BridgeError> {
        let payload = serde_json::to_string(&command)?;
        self.stdin.write_all(payload.as_bytes()).await?;
        self.stdin.write_all(b"\n").await?;
        self.stdin.flush().await?;

        let line = timeout(command_timeout, self.read_bridge_line())
            .await
            .map_err(|_| BridgeError::CommandTimeout {
                timeout: command_timeout,
            })??;
        let response: BridgeResponseMessage = serde_json::from_str(&line)?;
        if response.event != "bridge_response" {
            return Err(BridgeError::Protocol(format!(
                "expected bridge_response event, got {}",
                response.event
            )));
        }
        Ok(response)
    }

    async fn read_bridge_line(&mut self) -> Result<String, BridgeError> {
        let mut line = String::new();
        let bytes_read = self.stdout.read_line(&mut line).await?;
        if bytes_read == 0 {
            return Err(BridgeError::ChildClosed);
        }
        Ok(line)
    }

    async fn force_kill_child(&mut self) -> Result<(), BridgeError> {
        if self.child.try_wait()?.is_none() {
            self.child.kill().await?;
        }
        let _ = self.child.wait().await?;
        Ok(())
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
