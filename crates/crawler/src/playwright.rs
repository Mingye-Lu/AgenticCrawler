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

async function bootstrap() {
  const browser = await playwright.chromium.launch({ headless: true });
  const page = await browser.newPage();
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
        await page.goto(command.url, { waitUntil: 'domcontentloaded' });
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
            | Self::ShutdownTimeout { .. } => None,
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

        match timeout(launch_timeout, bridge.read_bootstrap_message()).await {
            Ok(result) => result?,
            Err(_) => {
                bridge.force_kill_child().await?;
                return Err(PlaywrightBridgeError::LaunchTimeout {
                    timeout: launch_timeout,
                });
            }
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

        match timeout(DEFAULT_SHUTDOWN_TIMEOUT, self.child.wait()).await {
            Ok(wait_result) => {
                let _status = wait_result?;
                Ok(())
            }
            Err(_) => {
                self.force_kill_child().await?;
                Err(PlaywrightBridgeError::ShutdownTimeout {
                    timeout: DEFAULT_SHUTDOWN_TIMEOUT,
                })
            }
        }
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

        let line = self.read_bridge_line().await?;
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
