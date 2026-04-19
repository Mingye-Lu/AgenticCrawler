use std::fmt;
use std::io;
use std::process::Stdio;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::time::timeout;

const DEFAULT_LAUNCH_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);
const DEFAULT_COMMAND_TIMEOUT: Duration = Duration::from_secs(60);

#[allow(clippy::needless_raw_string_hashes)]
const PLAYWRIGHT_BRIDGE_NODE_SCRIPT: &str = r#"
const readline = require('node:readline');

let playwright;
try {
  playwright = require('playwright');
} catch (_error) {
  process.stdout.write(JSON.stringify({
    event: 'bridge_bootstrap',
    ok: false,
    error: {
      kind: 'playwright_not_installed',
      message: 'Playwright package not found. Install with `npm install playwright` and browser binaries with `npx playwright install chromium`.'
    }
  }) + '\n');
  process.exit(1);
}

function parseHeadless() {
  const raw = process.env.HEADLESS;
  if (raw === undefined) return true;
  const v = String(raw).trim().toLowerCase();
  return !(v === 'false' || v === '0' || v === 'no' || v === 'off');
}

async function bootstrap() {
  const browser = await playwright.chromium.launch({ headless: parseHeadless() });
  let page = await browser.newPage();
  const pages = [page];
  const context = browser.contexts()[0];
  context.on('page', (p) => { pages.push(p); });
  process.stdout.write(JSON.stringify({ event: 'bridge_bootstrap', ok: true }) + '\n');

  const wire = readline.createInterface({ input: process.stdin, crlfDelay: Infinity });
  for await (const line of wire) {
    let command;
    try {
      command = JSON.parse(line);
    } catch (error) {
      process.stdout.write(JSON.stringify({
        event: 'bridge_response',
        ok: false,
        error: { kind: 'invalid_json', message: String(error) }
      }) + '\n');
      continue;
    }

    if (command.action === 'navigate') {
      try {
        await page.goto(command.url, { waitUntil: 'domcontentloaded', timeout: 30000 });
        const title = await page.title();
        const html = await page.content();
        process.stdout.write(JSON.stringify({
          event: 'bridge_response',
          ok: true,
          result: { title, html }
        }) + '\n');
      } catch (error) {
        process.stdout.write(JSON.stringify({
          event: 'bridge_response',
          ok: false,
          error: { kind: 'navigate_failed', message: String(error) }
        }) + '\n');
      }
      continue;
    }

    if (command.action === 'close') {
      await page.close().catch(() => {});
      await browser.close().catch(() => {});
      process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: true, result: { closed: true } }) + '\n');
      process.exit(0);
    }

    if (command.action === 'click') {
      try {
        await page.click(command.selector, { timeout: 5000 });
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: true, result: { clicked: true } }) + '\n');
      } catch (error) {
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: false, error: { kind: 'click_failed', message: String(error) } }) + '\n');
      }
      continue;
    }

    if (command.action === 'fill') {
      try {
        await page.fill(command.selector, command.value);
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: true, result: { filled: true } }) + '\n');
      } catch (error) {
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: false, error: { kind: 'fill_failed', message: String(error) } }) + '\n');
      }
      continue;
    }

    if (command.action === 'screenshot') {
      try {
        const buffer = await page.screenshot({ type: 'png' });
        const base64Data = buffer.toString('base64');
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: true, result: { screenshot_base64: base64Data, size_bytes: buffer.length } }) + '\n');
      } catch (error) {
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: false, error: { kind: 'screenshot_failed', message: String(error) } }) + '\n');
      }
      continue;
    }

    if (command.action === 'go_back') {
      try {
        await page.goBack({ waitUntil: 'domcontentloaded', timeout: 30000 });
        const url = page.url();
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: true, result: { url } }) + '\n');
      } catch (error) {
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: false, error: { kind: 'go_back_failed', message: String(error) } }) + '\n');
      }
      continue;
    }

    if (command.action === 'scroll') {
      try {
        const dir = command.direction === 'up' ? -1 : 1;
        const px = (command.pixels || 500) * dir;
        await page.evaluate((y) => window.scrollBy(0, y), px);
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: true, result: { scrolled: true } }) + '\n');
      } catch (error) {
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: false, error: { kind: 'scroll_failed', message: String(error) } }) + '\n');
      }
      continue;
    }

    if (command.action === 'wait_for_selector') {
      try {
        const timeout = command.timeout_ms || 5000;
        await page.waitForSelector(command.selector, { timeout });
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: true, result: { found: true } }) + '\n');
      } catch (error) {
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: true, result: { found: false } }) + '\n');
      }
      continue;
    }

    if (command.action === 'select_option') {
      try {
        await page.selectOption(command.selector, command.value);
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: true, result: { success: true } }) + '\n');
      } catch (error) {
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: false, error: { kind: 'select_option_failed', message: String(error) } }) + '\n');
      }
      continue;
    }

    if (command.action === 'evaluate') {
      try {
        const result = await page.evaluate(command.script);
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: true, result: { value: result } }) + '\n');
      } catch (error) {
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: false, error: { kind: 'evaluate_failed', message: String(error) } }) + '\n');
      }
      continue;
    }

    if (command.action === 'hover') {
      try {
        await page.hover(command.selector);
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: true, result: { success: true } }) + '\n');
      } catch (error) {
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: false, error: { kind: 'hover_failed', message: String(error) } }) + '\n');
      }
      continue;
    }

    if (command.action === 'press_key') {
      try {
        if (command.selector) {
          await page.focus(command.selector);
        }
        await page.keyboard.press(command.key);
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: true, result: { success: true } }) + '\n');
      } catch (error) {
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: false, error: { kind: 'press_key_failed', message: String(error) } }) + '\n');
      }
      continue;
    }

    if (command.action === 'list_resources') {
      try {
        const resources = await page.evaluate(() => {
          const links = Array.from(document.querySelectorAll('a[href]')).map(a => ({ href: a.href, text: a.textContent.trim() }));
          const images = Array.from(document.querySelectorAll('img')).map(img => ({ src: img.src, alt: img.alt }));
          const forms = Array.from(document.querySelectorAll('form')).map(f => ({ action: f.action, method: f.method, id: f.id }));
          return { links, images, forms };
        });
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: true, result: resources }) + '\n');
      } catch (error) {
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: false, error: { kind: 'list_resources_failed', message: String(error) } }) + '\n');
      }
      continue;
    }

    if (command.action === 'save_file') {
      try {
        const fs = require('node:fs');
        const nodePath = require('node:path');
        const resp = await page.evaluate(async (url) => {
          const r = await fetch(url);
          const buf = await r.arrayBuffer();
          return Array.from(new Uint8Array(buf));
        }, command.url);
        const dir = nodePath.dirname(command.path);
        if (!fs.existsSync(dir)) fs.mkdirSync(dir, { recursive: true });
        fs.writeFileSync(command.path, Buffer.from(resp));
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: true, result: { path: command.path } }) + '\n');
      } catch (error) {
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: false, error: { kind: 'save_file_failed', message: String(error) } }) + '\n');
      }
      continue;
    }

    if (command.action === 'switch_tab') {
      try {
        const idx = command.index === undefined ? -1 : command.index;
        const targetIdx = idx === -1 ? pages.length - 1 : idx;
        if (targetIdx < 0 || targetIdx >= pages.length) {
          process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: false, error: { kind: 'switch_tab_failed', message: `Invalid tab index ${idx}, have ${pages.length} tab(s)` } }) + '\n');
        } else {
          page = pages[targetIdx];
          await page.bringToFront();
          const url = page.url();
          const title = await page.title();
          process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: true, result: { url, title, tab_count: pages.length } }) + '\n');
        }
      } catch (error) {
        process.stdout.write(JSON.stringify({ event: 'bridge_response', ok: false, error: { kind: 'switch_tab_failed', message: String(error) } }) + '\n');
      }
      continue;
    }

    process.stdout.write(JSON.stringify({
      event: 'bridge_response',
      ok: false,
      error: { kind: 'unsupported_action', message: `Unsupported action: ${command.action}` }
    }) + '\n');
  }
}

bootstrap().catch((error) => {
  process.stdout.write(JSON.stringify({
    event: 'bridge_bootstrap',
    ok: false,
    error: { kind: 'launch_failed', message: String(error) }
  }) + '\n');
  process.exit(1);
});
"#;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PageInfo {
    pub title: String,
    pub html: String,
}

#[derive(Debug)]
pub enum PlaywrightBridgeError {
    ProcessSpawn { command: String, source: io::Error },
    LaunchTimeout { timeout: Duration },
    Protocol(String),
    PlaywrightNotInstalled(String),
    Io(io::Error),
    Json(serde_json::Error),
    ChildClosed,
    ShutdownTimeout { timeout: Duration },
    CommandTimeout { timeout: Duration },
}

impl fmt::Display for PlaywrightBridgeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ProcessSpawn { command, source } => write!(
                f,
                "failed to spawn `{command}` for Playwright bridge: {source}. Ensure Node.js and Playwright are installed"
            ),
            Self::LaunchTimeout { timeout } => write!(
                f,
                "Playwright bridge launch exceeded {} seconds",
                timeout.as_secs()
            ),
            Self::Protocol(message) => write!(f, "Playwright bridge protocol error: {message}"),
            Self::PlaywrightNotInstalled(message) => write!(
                f,
                "Playwright is not installed: {message}. Install with `npm install playwright` and `npx playwright install chromium`"
            ),
            Self::Io(error) => write!(f, "Playwright bridge I/O error: {error}"),
            Self::Json(error) => write!(f, "Playwright bridge JSON error: {error}"),
            Self::ChildClosed => write!(f, "Playwright bridge process closed unexpectedly"),
            Self::ShutdownTimeout { timeout } => write!(
                f,
                "Playwright bridge did not shut down within {} seconds",
                timeout.as_secs()
            ),
            Self::CommandTimeout { timeout } => write!(
                f,
                "Playwright bridge command timed out after {} seconds",
                timeout.as_secs()
            ),
        }
    }
}

impl std::error::Error for PlaywrightBridgeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ProcessSpawn { source, .. } => Some(source),
            Self::Io(error) => Some(error),
            Self::Json(error) => Some(error),
            Self::LaunchTimeout { .. }
            | Self::Protocol(_)
            | Self::PlaywrightNotInstalled(_)
            | Self::ChildClosed
            | Self::ShutdownTimeout { .. }
            | Self::CommandTimeout { .. } => None,
        }
    }
}

impl From<io::Error> for PlaywrightBridgeError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<serde_json::Error> for PlaywrightBridgeError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

#[derive(Debug)]
pub struct PlaywrightBridge {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl PlaywrightBridge {
    pub async fn new() -> Result<Self, PlaywrightBridgeError> {
        Self::new_with_invocation(
            "node",
            vec!["-e".to_string(), PLAYWRIGHT_BRIDGE_NODE_SCRIPT.to_string()],
            DEFAULT_LAUNCH_TIMEOUT,
        )
        .await
    }

    async fn new_with_invocation(
        program: &str,
        args: Vec<String>,
        launch_timeout: Duration,
    ) -> Result<Self, PlaywrightBridgeError> {
        let mut command = Command::new(program);
        command
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true);

        let mut child = command
            .spawn()
            .map_err(|source| PlaywrightBridgeError::ProcessSpawn {
                command: format!("{program} {}", args.join(" ")),
                source,
            })?;

        let stdin = child.stdin.take().ok_or_else(|| {
            PlaywrightBridgeError::Protocol("bridge process missing stdin pipe".to_string())
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            PlaywrightBridgeError::Protocol("bridge process missing stdout pipe".to_string())
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
            return Err(PlaywrightBridgeError::LaunchTimeout {
                timeout: launch_timeout,
            });
        }

        Ok(bridge)
    }

    pub async fn navigate(&mut self, url: &str) -> Result<PageInfo, PlaywrightBridgeError> {
        let response = self
            .send_bridge_command(BridgeCommandEnvelope {
                action: "navigate",
                url: Some(url),
            })
            .await?;

        if response.ok {
            return response.result.ok_or_else(|| {
                PlaywrightBridgeError::Protocol(
                    "navigate response missing page result payload".to_string(),
                )
            });
        }

        let error = response.error.ok_or_else(|| {
            PlaywrightBridgeError::Protocol("navigate response missing error payload".to_string())
        })?;
        Err(PlaywrightBridgeError::Protocol(format!(
            "{}: {}",
            error.kind, error.message
        )))
    }

    pub async fn close(mut self) -> Result<(), PlaywrightBridgeError> {
        let _ = self
            .send_bridge_command(BridgeCommandEnvelope {
                action: "close",
                url: None,
            })
            .await;

        if let Ok(wait_result) = timeout(DEFAULT_SHUTDOWN_TIMEOUT, self.child.wait()).await {
            let _status = wait_result?;
            Ok(())
        } else {
            self.force_kill_child().await?;
            Err(PlaywrightBridgeError::ShutdownTimeout {
                timeout: DEFAULT_SHUTDOWN_TIMEOUT,
            })
        }
    }

    pub async fn send_raw_command(
        &mut self,
        command: &serde_json::Value,
    ) -> Result<serde_json::Value, PlaywrightBridgeError> {
        let payload = serde_json::to_string(command)?;
        self.stdin.write_all(payload.as_bytes()).await?;
        self.stdin.write_all(b"\n").await?;
        self.stdin.flush().await?;

        let line = timeout(DEFAULT_COMMAND_TIMEOUT, self.read_bridge_line())
            .await
            .map_err(|_| PlaywrightBridgeError::CommandTimeout {
                timeout: DEFAULT_COMMAND_TIMEOUT,
            })??;
        let response: GenericBridgeResponseMessage = serde_json::from_str(&line)?;
        if response.event != "bridge_response" {
            return Err(PlaywrightBridgeError::Protocol(format!(
                "expected bridge_response event, got {}",
                response.event
            )));
        }
        if response.ok {
            return Ok(response.result.unwrap_or(serde_json::Value::Null));
        }
        let error = response.error.ok_or_else(|| {
            PlaywrightBridgeError::Protocol("response missing error payload".to_string())
        })?;
        Err(PlaywrightBridgeError::Protocol(format!(
            "{}: {}",
            error.kind, error.message
        )))
    }

    pub async fn scroll(
        &mut self,
        direction: &str,
        pixels: i64,
    ) -> Result<(), PlaywrightBridgeError> {
        let cmd = serde_json::json!({
            "action": "scroll",
            "direction": direction,
            "pixels": pixels,
        });
        self.send_raw_command(&cmd).await?;
        Ok(())
    }

    pub async fn wait_for_selector(
        &mut self,
        selector: &str,
        timeout_ms: u64,
    ) -> Result<bool, PlaywrightBridgeError> {
        let cmd = serde_json::json!({
            "action": "wait_for_selector",
            "selector": selector,
            "timeout_ms": timeout_ms,
        });
        let result = self.send_raw_command(&cmd).await?;
        Ok(result
            .get("found")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false))
    }

    pub async fn select_option(
        &mut self,
        selector: &str,
        value: &str,
    ) -> Result<(), PlaywrightBridgeError> {
        let cmd = serde_json::json!({
            "action": "select_option",
            "selector": selector,
            "value": value,
        });
        self.send_raw_command(&cmd).await?;
        Ok(())
    }

    pub async fn evaluate(
        &mut self,
        script: &str,
    ) -> Result<serde_json::Value, PlaywrightBridgeError> {
        let cmd = serde_json::json!({
            "action": "evaluate",
            "script": script,
        });
        self.send_raw_command(&cmd).await
    }

    pub async fn hover(&mut self, selector: &str) -> Result<(), PlaywrightBridgeError> {
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
    ) -> Result<(), PlaywrightBridgeError> {
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

    pub async fn switch_tab(
        &mut self,
        index: i64,
    ) -> Result<serde_json::Value, PlaywrightBridgeError> {
        let cmd = serde_json::json!({
            "action": "switch_tab",
            "index": index,
        });
        self.send_raw_command(&cmd).await
    }

    pub async fn list_resources(&mut self) -> Result<serde_json::Value, PlaywrightBridgeError> {
        let cmd = serde_json::json!({ "action": "list_resources" });
        self.send_raw_command(&cmd).await
    }

    pub async fn save_file(
        &mut self,
        url: &str,
        path: &str,
    ) -> Result<String, PlaywrightBridgeError> {
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

    pub async fn click(&mut self, selector: &str) -> Result<(), PlaywrightBridgeError> {
        let cmd = serde_json::json!({
            "action": "click",
            "selector": selector,
        });
        self.send_raw_command(&cmd).await?;
        Ok(())
    }

    pub async fn fill(&mut self, selector: &str, value: &str) -> Result<(), PlaywrightBridgeError> {
        let cmd = serde_json::json!({
            "action": "fill",
            "selector": selector,
            "value": value,
        });
        self.send_raw_command(&cmd).await?;
        Ok(())
    }

    pub async fn screenshot(&mut self) -> Result<(String, usize), PlaywrightBridgeError> {
        let cmd = serde_json::json!({ "action": "screenshot" });
        let result = self.send_raw_command(&cmd).await?;
        let base64_data = result
            .get("screenshot_base64")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| {
                PlaywrightBridgeError::Protocol(
                    "screenshot response missing base64 data".to_string(),
                )
            })?
            .to_string();
        #[allow(clippy::cast_possible_truncation)]
        let size_bytes = result
            .get("size_bytes")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0) as usize;
        Ok((base64_data, size_bytes))
    }

    pub async fn go_back(&mut self) -> Result<String, PlaywrightBridgeError> {
        let cmd = serde_json::json!({ "action": "go_back" });
        let result = self.send_raw_command(&cmd).await?;
        let url = result
            .get("url")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_string();
        Ok(url)
    }

    async fn read_bootstrap_message(&mut self) -> Result<(), PlaywrightBridgeError> {
        let line = self.read_bridge_line().await?;
        let message: BridgeBootstrapMessage = serde_json::from_str(&line)?;

        if message.event != "bridge_bootstrap" {
            return Err(PlaywrightBridgeError::Protocol(format!(
                "expected bridge_bootstrap event, got {}",
                message.event
            )));
        }

        if message.ok {
            return Ok(());
        }

        let error = message.error.ok_or_else(|| {
            PlaywrightBridgeError::Protocol("bootstrap failure missing error payload".to_string())
        })?;

        if error.kind == "playwright_not_installed" {
            return Err(PlaywrightBridgeError::PlaywrightNotInstalled(error.message));
        }

        Err(PlaywrightBridgeError::Protocol(format!(
            "bootstrap failed: {} ({})",
            error.message, error.kind
        )))
    }

    async fn send_bridge_command(
        &mut self,
        command: BridgeCommandEnvelope<'_>,
    ) -> Result<BridgeResponseMessage, PlaywrightBridgeError> {
        let payload = serde_json::to_string(&command)?;
        self.stdin.write_all(payload.as_bytes()).await?;
        self.stdin.write_all(b"\n").await?;
        self.stdin.flush().await?;

        let line = timeout(DEFAULT_COMMAND_TIMEOUT, self.read_bridge_line())
            .await
            .map_err(|_| PlaywrightBridgeError::CommandTimeout {
                timeout: DEFAULT_COMMAND_TIMEOUT,
            })??;
        let response: BridgeResponseMessage = serde_json::from_str(&line)?;
        if response.event != "bridge_response" {
            return Err(PlaywrightBridgeError::Protocol(format!(
                "expected bridge_response event, got {}",
                response.event
            )));
        }
        Ok(response)
    }

    async fn read_bridge_line(&mut self) -> Result<String, PlaywrightBridgeError> {
        let mut line = String::new();
        let bytes_read = self.stdout.read_line(&mut line).await?;
        if bytes_read == 0 {
            return Err(PlaywrightBridgeError::ChildClosed);
        }
        Ok(line)
    }

    async fn force_kill_child(&mut self) -> Result<(), PlaywrightBridgeError> {
        if self.child.try_wait()?.is_none() {
            self.child.kill().await?;
        }
        let _ = self.child.wait().await?;
        Ok(())
    }
}

#[derive(Debug, Serialize)]
struct BridgeCommandEnvelope<'a> {
    action: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    url: Option<&'a str>,
}

#[derive(Debug, Deserialize)]
struct BridgeBootstrapMessage {
    event: String,
    ok: bool,
    #[serde(default)]
    error: Option<BridgeErrorPayload>,
}

#[derive(Debug, Deserialize)]
struct BridgeResponseMessage {
    event: String,
    ok: bool,
    #[serde(default)]
    result: Option<PageInfo>,
    #[serde(default)]
    error: Option<BridgeErrorPayload>,
}

/// Generic bridge response that deserializes `result` as arbitrary JSON.
#[derive(Debug, Deserialize)]
struct GenericBridgeResponseMessage {
    event: String,
    ok: bool,
    #[serde(default)]
    result: Option<serde_json::Value>,
    #[serde(default)]
    error: Option<BridgeErrorPayload>,
}

#[derive(Debug, Deserialize)]
struct BridgeErrorPayload {
    kind: String,
    message: String,
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use super::{PageInfo, PlaywrightBridge, PlaywrightBridgeError};

    fn temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("crawler-playwright-{prefix}-{nanos}"))
    }

    fn cleanup_temp_script(path: &Path) {
        let _ = fs::remove_file(path);
        if let Some(parent) = path.parent() {
            let _ = fs::remove_dir_all(parent);
        }
    }

    fn python_program() -> &'static str {
        if cfg!(windows) {
            "python"
        } else {
            "python3"
        }
    }

    fn write_python_script(name: &str, source: &str) -> PathBuf {
        let root = temp_dir(name);
        fs::create_dir_all(&root).expect("create temp dir");
        let path = root.join(format!("{name}.py"));
        fs::write(&path, source).expect("write temp script");
        path
    }

    #[tokio::test]
    async fn returns_descriptive_error_when_playwright_is_missing() {
        let script_path = write_python_script(
            "missing-playwright",
            r#"import json
import sys
print(json.dumps({
    "event": "bridge_bootstrap",
    "ok": False,
    "error": {
        "kind": "playwright_not_installed",
        "message": "module 'playwright' missing"
    }
}), flush=True)
sys.exit(1)
"#,
        );

        let result = PlaywrightBridge::new_with_invocation(
            python_program(),
            vec![script_path.to_string_lossy().into_owned()],
            Duration::from_secs(2),
        )
        .await;

        cleanup_temp_script(&script_path);

        match result {
            Err(PlaywrightBridgeError::PlaywrightNotInstalled(message)) => {
                assert!(message.contains("playwright"));
            }
            other => panic!("expected PlaywrightNotInstalled error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn returns_launch_timeout_when_bootstrap_takes_too_long() {
        let script_path = write_python_script(
            "slow-bootstrap",
            r#"import time
time.sleep(2)
print('{"event":"bridge_bootstrap","ok":true}', flush=True)
"#,
        );

        let result = PlaywrightBridge::new_with_invocation(
            python_program(),
            vec![script_path.to_string_lossy().into_owned()],
            Duration::from_millis(150),
        )
        .await;

        cleanup_temp_script(&script_path);

        match result {
            Err(PlaywrightBridgeError::LaunchTimeout { .. }) => {}
            other => panic!("expected LaunchTimeout error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn navigate_round_trip_works_over_json_lines_protocol() {
        let script_path = write_python_script(
            "protocol-server",
            r#"import json
import sys
print(json.dumps({"event": "bridge_bootstrap", "ok": True}), flush=True)
for line in sys.stdin:
    command = json.loads(line)
    if command.get("action") == "navigate":
        print(json.dumps({
            "event": "bridge_response",
            "ok": True,
            "result": {
                "title": "Synthetic Example",
                "html": "<html><title>Synthetic Example</title></html>"
            }
        }), flush=True)
    elif command.get("action") == "close":
        print(json.dumps({"event": "bridge_response", "ok": True}), flush=True)
        break
"#,
        );

        let mut bridge = PlaywrightBridge::new_with_invocation(
            python_program(),
            vec![script_path.to_string_lossy().into_owned()],
            Duration::from_secs(2),
        )
        .await
        .expect("bridge should bootstrap");

        let page = bridge
            .navigate("https://example.com")
            .await
            .expect("navigate should succeed");
        assert_eq!(
            page,
            PageInfo {
                title: "Synthetic Example".to_string(),
                html: "<html><title>Synthetic Example</title></html>".to_string(),
            }
        );

        bridge.close().await.expect("close should succeed");
        cleanup_temp_script(&script_path);
    }

    #[tokio::test]
    #[ignore = "requires node + playwright installed locally"]
    async fn ignored_real_playwright_navigate_example_com() {
        let mut bridge = PlaywrightBridge::new()
            .await
            .expect("playwright bridge should launch");
        let page = bridge
            .navigate("https://example.com")
            .await
            .expect("navigate should succeed");

        assert!(page.title.contains("Example Domain"));
        assert!(page.html.contains("Example Domain"));

        bridge
            .close()
            .await
            .expect("bridge should close without zombie process");
    }
}
