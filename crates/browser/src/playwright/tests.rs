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
fn page_map_command_forwards_scope_enrichment_and_depth() {
    assert_eq!(
        build_page_map_command(None, false, None),
        serde_json::json!({ "action": "page_map" })
    );
    assert_eq!(
        build_page_map_command(Some("main"), true, Some(7)),
        serde_json::json!({
            "action": "page_map",
            "scope": "main",
            "compoundEnrichment": true,
            "depth": 7
        })
    );
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
            html: Some("<html><title>Synthetic Example</title></html>".to_string()),
        }
    );

    bridge.close().await.expect("close should succeed");
    cleanup_temp_script(&script_path);
}

#[tokio::test]
async fn command_timeout_does_not_desync_next_response() {
    // Regression test for issue #69: a command whose read times out client-side
    // still gets a response from the bridge process eventually (it's just late).
    // If the next command's read isn't protected, it swallows that stale
    // response instead of its own, corrupting whatever it returns.
    let script_path = write_python_script(
        "timeout-desync",
        r#"import json
import sys
import time
print(json.dumps({"event": "bridge_bootstrap", "ok": True}), flush=True)
for line in sys.stdin:
    command = json.loads(line)
    action = command.get("action")
    if action == "slow_action":
        time.sleep(0.3)
        print(json.dumps({
            "event": "bridge_response",
            "ok": True,
            "result": {"title": "stale", "html": "<stale></stale>"}
        }), flush=True)
    elif action == "navigate":
        print(json.dumps({
            "event": "bridge_response",
            "ok": True,
            "result": {"title": "fresh", "html": "<fresh></fresh>"}
        }), flush=True)
    elif action == "close":
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

    let timed_out = bridge
        .send_bridge_command_with_timeout(
            BridgeCommandEnvelope {
                action: "slow_action",
                url: None,
            },
            Duration::from_millis(50),
        )
        .await;
    assert!(
        matches!(timed_out, Err(BridgeError::CommandTimeout { .. })),
        "expected CommandTimeout, got {timed_out:?}"
    );

    // Let the slow response actually land on the pipe before issuing the next
    // command, so the desync window is real rather than a timing accident.
    tokio::time::sleep(Duration::from_millis(400)).await;

    let page = bridge
        .navigate("https://example.com")
        .await
        .expect("navigate should succeed after a prior command timed out");
    assert_eq!(
        page,
        PageInfo {
            title: "fresh".to_string(),
            html: Some("<fresh></fresh>".to_string()),
        },
        "navigate must not receive the stale slow_action response"
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
    assert!(page.html.unwrap_or_default().contains("Example Domain"));

    bridge
        .close()
        .await
        .expect("bridge should close without zombie process");
}

#[tokio::test]
#[ignore = "requires node + playwright installed locally"]
async fn close_page_shrinks_tab_count_and_keeps_indices_consistent() {
    // Regression test for issue #69: closing a non-last tab used to leave a
    // null hole in the pages array instead of removing it, so tab_count never
    // shrank and switch_tab/new_page saw stale indices.
    let mut bridge = PlaywrightBridge::new()
        .await
        .expect("playwright bridge should launch");

    let idx1 = bridge
        .new_page(Some("https://example.com"))
        .await
        .expect("new_page 1 should succeed");
    let idx2 = bridge
        .new_page(Some("https://example.org"))
        .await
        .expect("new_page 2 should succeed");
    assert_eq!(idx1, 1);
    assert_eq!(idx2, 2);

    // Close the middle tab (not the last), which previously left a hole.
    bridge
        .close_page(1)
        .await
        .expect("close_page should succeed");

    // What was tab 2 should now be reachable at index 1, and tab_count should
    // have shrunk to 2 rather than staying at 3 with a dead slot.
    let tab = bridge
        .switch_tab(1)
        .await
        .expect("switch_tab should reach the shifted former index-2 tab");
    assert_eq!(
        tab.get("tab_count").and_then(serde_json::Value::as_u64),
        Some(2),
        "closing a tab should shrink tab_count instead of leaving a hole"
    );
    assert_eq!(
        tab.get("url").and_then(serde_json::Value::as_str),
        Some("https://example.org/")
    );

    // navigate should still work normally after a close_page (the other half
    // of the original bug report: navigation intermittently failed with a
    // missing-html bridge error).
    let page = bridge
        .navigate("https://example.com")
        .await
        .expect("navigate should succeed after close_page");
    assert!(page.html.unwrap_or_default().contains("Example Domain"));

    bridge
        .close()
        .await
        .expect("bridge should close without zombie process");
}

#[tokio::test]
#[ignore = "requires node + playwright installed locally"]
async fn observation_events_track_correct_tab_after_close_page() {
    // Regression test: attachObservationListeners used to capture a page's
    // index once at attach time, so after close_page shifted indices, a
    // surviving tab's network events kept landing under its stale pre-close
    // index instead of the index observationBuffers was re-keyed to.
    let mut bridge = PlaywrightBridge::new()
        .await
        .expect("playwright bridge should launch");

    let idx1 = bridge
        .new_page(Some("https://example.com"))
        .await
        .expect("new_page 1 should succeed");
    let idx2 = bridge
        .new_page(Some("https://example.org"))
        .await
        .expect("new_page 2 should succeed");
    assert_eq!(idx1, 1);
    assert_eq!(idx2, 2);

    // Drop the events buffered for all tabs up to this point.
    for i in 0..3 {
        bridge
            .poll_observations_raw(i)
            .await
            .expect("poll_observations should succeed before close");
    }

    // Close tab 0, shifting tab 1 -> 0 and tab 2 -> 1.
    bridge
        .close_page(0)
        .await
        .expect("close_page should succeed");

    // Generate a fresh network request on the tab now at index 1 (formerly
    // index 2, https://example.org) by switching to it and reloading.
    bridge
        .switch_tab(1)
        .await
        .expect("switch_tab to former index-2 tab should succeed");
    bridge.reload().await.expect("reload should succeed");

    let events_at_new_index = bridge
        .poll_observations_raw(1)
        .await
        .expect("poll_observations at new index should succeed");
    let saw_example_org_request = events_at_new_index.iter().any(|event| {
        event
            .get("url")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|url| url.contains("example.org"))
    });
    assert!(
        saw_example_org_request,
        "expected reload's network events for the surviving example.org tab \
         to be buffered under its new post-close index 1, got: {events_at_new_index:?}"
    );

    let events_at_stale_index = bridge
        .poll_observations_raw(2)
        .await
        .expect("poll_observations at stale index should succeed");
    assert!(
        events_at_stale_index.iter().all(|event| {
            event
                .get("url")
                .and_then(serde_json::Value::as_str)
                .is_none_or(|url| !url.contains("example.org"))
        }),
        "example.org's events must not still be filed under its old \
         pre-close index 2, got: {events_at_stale_index:?}"
    );

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

#[test]
fn classify_io_error_maps_broken_pipe_to_child_closed() {
    let err = std::io::Error::from(std::io::ErrorKind::BrokenPipe);
    assert!(matches!(classify_io_error(err), BridgeError::ChildClosed));
}

#[test]
fn classify_io_error_maps_windows_pipe_closing_to_child_closed() {
    let err = std::io::Error::from_raw_os_error(232);
    assert!(matches!(classify_io_error(err), BridgeError::ChildClosed));
}

#[test]
fn classify_io_error_preserves_unrelated_errors() {
    let err = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
    assert!(matches!(classify_io_error(err), BridgeError::Io(_)));
}

#[test]
fn normalize_intercept_pattern_promotes_single_star_to_double() {
    let (pattern, is_regex) = normalize_intercept_pattern("*.ads.com/*");
    assert_eq!(pattern, "**.ads.com/**");
    assert!(!is_regex);
}

#[test]
fn normalize_intercept_pattern_collapses_star_runs() {
    let (pattern, _) = normalize_intercept_pattern("**/api/b*");
    assert_eq!(pattern, "**/api/b**");
}

#[test]
fn normalize_intercept_pattern_detects_regex_prefix() {
    let (pattern, is_regex) = normalize_intercept_pattern("re:api/v[0-9]+");
    assert_eq!(pattern, "api/v[0-9]+");
    assert!(is_regex);
}

#[test]
fn normalize_intercept_pattern_leaves_starless_literals_untouched() {
    let (pattern, is_regex) = normalize_intercept_pattern("https://example.com/api/v2");
    assert_eq!(pattern, "https://example.com/api/v2");
    assert!(!is_regex);
}
