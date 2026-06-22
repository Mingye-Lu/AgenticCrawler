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

fn classify_io_error(err: std::io::Error) -> BridgeError {
    use std::io::ErrorKind;
    let pipe_closed = matches!(
        err.kind(),
        ErrorKind::BrokenPipe
            | ErrorKind::ConnectionReset
            | ErrorKind::ConnectionAborted
            | ErrorKind::UnexpectedEof
    ) || err.raw_os_error() == Some(232);
    if pipe_closed {
        BridgeError::ChildClosed
    } else {
        BridgeError::Io(err)
    }
}

// Playwright globs treat bare `*` as `[^/]*` (it does not cross `/`), so common
// block patterns like `*.ads.com/*` silently match nothing. Collapse `*` runs to
// `**` so wildcards span path separators; a `re:` prefix opts into a raw regex.
fn normalize_intercept_pattern(pattern: &str) -> (String, bool) {
    if let Some(rest) = pattern.strip_prefix("re:") {
        return (rest.to_string(), true);
    }
    let mut out = String::with_capacity(pattern.len() + 2);
    let mut in_star_run = false;
    for c in pattern.chars() {
        if c == '*' {
            if !in_star_run {
                out.push_str("**");
                in_star_run = true;
            }
        } else {
            out.push(c);
            in_star_run = false;
        }
    }
    (out, false)
}

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
        self.stdin
            .write_all(payload.as_bytes())
            .await
            .map_err(classify_io_error)?;
        self.stdin
            .write_all(b"\n")
            .await
            .map_err(classify_io_error)?;
        self.stdin.flush().await.map_err(classify_io_error)?;

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

    pub async fn extract_dom_snapshot(
        &mut self,
        scope: Option<&str>,
    ) -> Result<serde_json::Value, BridgeError> {
        let mut cmd = serde_json::json!({ "action": "extract_dom_snapshot" });
        if let Some(s) = scope {
            cmd["scope"] = serde_json::Value::String(s.to_string());
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

    pub async fn get_cookies(&mut self) -> Result<Vec<crate::CookieInfo>, BridgeError> {
        let cmd = serde_json::json!({ "action": "get_cookies" });
        let result = self.send_raw_command(&cmd).await?;
        let cookies = result
            .get("cookies")
            .and_then(serde_json::Value::as_array)
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

    pub async fn get_storage(
        &mut self,
        storage_type: crate::StorageType,
    ) -> Result<(Vec<crate::StorageEntry>, Vec<crate::StorageEntry>), BridgeError> {
        let storage_type_str = match storage_type {
            crate::StorageType::Local => "local",
            crate::StorageType::Session => "session",
            crate::StorageType::All => "all",
        };
        let cmd = serde_json::json!({
            "action": "get_storage",
            "storage_type": storage_type_str,
        });
        let result = self.send_raw_command(&cmd).await?;

        let local_storage = result
            .get("local_storage")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default();
        let session_storage = result
            .get("session_storage")
            .and_then(serde_json::Value::as_array)
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

    pub async fn start_coverage(&mut self, js: bool, css: bool) -> Result<(), BridgeError> {
        let cmd = serde_json::json!({
            "action": "start_coverage",
            "js": js,
            "css": css,
        });
        self.send_raw_command(&cmd).await?;
        Ok(())
    }

    pub async fn stop_coverage(&mut self) -> Result<crate::CoverageData, BridgeError> {
        let cmd = serde_json::json!({ "action": "stop_coverage" });
        let result = self.send_raw_command(&cmd).await?;

        let js_entries = result
            .get("js_coverage")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default();
        let css_entries = result
            .get("css_coverage")
            .and_then(serde_json::Value::as_array)
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

    pub async fn reload(&mut self) -> Result<PageInfo, BridgeError> {
        let cmd = serde_json::json!({ "action": "reload" });
        let result = self.send_raw_command(&cmd).await?;
        let title = result
            .get("title")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_string();
        let html = result
            .get("html")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_string();
        Ok(PageInfo { title, html })
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

    pub async fn add_intercept_rule(
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
        let (pattern, is_regex) = normalize_intercept_pattern(&rule.pattern);
        self.send_raw_command(&serde_json::json!({
            "action": "add_intercept_rule",
            "rule_id": rule_id,
            "pattern": pattern,
            "is_regex": is_regex,
            "action_type": format!("{:?}", rule.action),
            "mock": rule.mock,
        }))
        .await?;
        Ok(rule_id)
    }

    pub async fn remove_intercept_rule(&mut self, rule_id: &str) -> Result<(), BridgeError> {
        self.send_raw_command(&serde_json::json!({
            "action": "remove_intercept_rule",
            "rule_id": rule_id,
        }))
        .await?;
        Ok(())
    }

    pub async fn clear_intercept_rules(&mut self) -> Result<(), BridgeError> {
        self.send_raw_command(&serde_json::json!({"action": "clear_intercept_rules"}))
            .await?;
        Ok(())
    }

    async fn read_bridge_line(&mut self) -> Result<String, BridgeError> {
        let mut line = String::new();
        let bytes_read = self
            .stdout
            .read_line(&mut line)
            .await
            .map_err(classify_io_error)?;
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
