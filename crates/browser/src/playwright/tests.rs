use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use acrawl_core::config_home_dir;

use super::super::BrowserState;
use super::*;

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

#[test]
fn bridge_command_sets_node_path_to_config_home_node_modules() {
    let args = vec!["-e".to_string(), "console.log('ok')".to_string()];
    let command = PlaywrightBridge::bridge_command("node", &args);
    let node_path = command.as_std().get_envs().find_map(|(key, value)| {
        if key == "NODE_PATH" {
            value.map(std::ffi::OsStr::to_os_string)
        } else {
            None
        }
    });

    let node_path = node_path.expect("NODE_PATH should be set");
    let paths: Vec<_> = std::env::split_paths(&node_path).collect();
    let expected = config_home_dir().join("node_modules");
    assert!(
        paths.contains(&expected),
        "NODE_PATH should contain {expected:?}, got {paths:?}"
    );
}

#[test]
fn bridge_command_uses_child_stderr_from_core() {
    acrawl_core::set_tui_active(false);
    let args = vec!["-e".to_string(), "1".to_string()];
    let cmd = PlaywrightBridge::bridge_command("node", &args);
    let debug = format!("{:?}", cmd.as_std());
    assert!(
        !debug.contains("\"null\""),
        "stderr should not be null when TUI inactive"
    );

    acrawl_core::set_tui_active(true);
    let cmd = PlaywrightBridge::bridge_command("node", &args);
    let debug = format!("{:?}", cmd.as_std());
    assert!(
        !debug.contains("\"inherit\""),
        "stderr should not be inherit when TUI active"
    );
    acrawl_core::set_tui_active(false);
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
        Err(BridgeError::PlaywrightNotInstalled(message)) => {
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
        Err(BridgeError::LaunchTimeout { .. }) => {}
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
async fn test_bridge_new_page_returns_index() {
    let mut bridge = PlaywrightBridge::new()
        .await
        .expect("playwright bridge should launch");

    let page_index = bridge
        .new_page(None)
        .await
        .expect("new_page should succeed");

    assert_eq!(page_index, 1);

    bridge
        .close()
        .await
        .expect("bridge should close without zombie process");
}

#[tokio::test]
#[ignore = "requires node + playwright installed locally"]
async fn test_bridge_close_page_success() {
    let mut bridge = PlaywrightBridge::new()
        .await
        .expect("playwright bridge should launch");

    let page_index = bridge
        .new_page(None)
        .await
        .expect("new_page should succeed");

    bridge
        .close_page(page_index)
        .await
        .expect("close_page should succeed");

    bridge
        .close()
        .await
        .expect("bridge should close without zombie process");
}

#[tokio::test]
#[ignore = "requires node + playwright installed locally"]
async fn test_bridge_new_page_with_url() {
    let mut bridge = PlaywrightBridge::new()
        .await
        .expect("playwright bridge should launch");

    let page_index = bridge
        .new_page(Some("https://example.com"))
        .await
        .expect("new_page should succeed");

    let tab = bridge
        .switch_tab(i64::try_from(page_index).expect("page_index fits i64"))
        .await
        .expect("switch_tab should succeed");

    assert_eq!(page_index, 1);
    assert_eq!(
        tab.get("url").and_then(serde_json::Value::as_str),
        Some("https://example.com/")
    );

    bridge
        .close()
        .await
        .expect("bridge should close without zombie process");
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

#[test]
fn browser_state_serializes_and_deserializes() {
    let state = BrowserState {
        cookies: serde_json::json!([{"name": "session", "value": "abc123", "domain": "example.com"}]),
        local_storage: serde_json::json!({"theme": "dark", "lang": "en"}),
        url: "https://example.com/page".to_string(),
    };
    let json = serde_json::to_string(&state).expect("should serialize");
    let parsed: BrowserState = serde_json::from_str(&json).expect("should deserialize");
    assert_eq!(parsed.url, state.url);
    assert_eq!(parsed.local_storage["theme"], "dark");
}

#[test]
fn browser_state_empty_cookies_round_trips() {
    let state = BrowserState {
        cookies: serde_json::json!([]),
        local_storage: serde_json::json!({}),
        url: String::new(),
    };
    let json = serde_json::to_string(&state).unwrap();
    let parsed: BrowserState = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.url, "");
}
