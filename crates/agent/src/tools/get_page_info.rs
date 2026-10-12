use serde_json::{json, Value};

use crate::state::CrawlState;
use crate::BridgeError;
use crate::BrowserContext;
use crate::{ToolEffect, ToolExecutionError};

/// Titles are clipped so a listing of many tabs stays a glance, not a page of text.
const MAX_TAB_TITLE_CHARS: usize = 120;

fn clip_title(title: &str) -> String {
    if title.chars().count() <= MAX_TAB_TITLE_CHARS {
        return title.to_string();
    }
    let head: String = title.chars().take(MAX_TAB_TITLE_CHARS - 1).collect();
    format!("{head}\u{2026}")
}

/// Shape the backend's `{"tabs": [{index, url, title}]}` for the agent and mark the tab it is
/// working on. The marker comes from the context's own `page_index`, not from the browser's
/// focused tab: a user clicking around Chrome does not move the agent.
fn tabs_json(raw: &Value, active_index: usize) -> Vec<Value> {
    raw.get("tabs")
        .and_then(Value::as_array)
        .map(|tabs| {
            tabs.iter()
                .filter_map(|tab| {
                    let index = tab.get("index").and_then(Value::as_u64)?;
                    Some(json!({
                        "index": index,
                        "url": tab.get("url").and_then(Value::as_str).unwrap_or_default(),
                        "title": clip_title(
                            tab.get("title").and_then(Value::as_str).unwrap_or_default()
                        ),
                        "active": usize::try_from(index).is_ok_and(|i| i == active_index),
                    }))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The string a `bridge.evaluate` produced, or the error text when the evaluation failed.
fn eval_string(result: Result<Value, BridgeError>) -> Result<Option<String>, String> {
    result
        .map(|v| {
            v.get("value")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        })
        .map_err(|e| e.to_string())
}

/// Read-only: never advances `seq` (see `increment_seq`) or changes the active tab.
pub async fn execute(
    input: &Value,
    browser: &mut BrowserContext,
    crawl_state: &mut CrawlState,
) -> Result<ToolEffect, ToolExecutionError> {
    let _ = crawl_state;
    let all_tabs = input
        .get("all_tabs")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    if !browser.is_browser_loaded() {
        let url = browser
            .current_url()
            .map(ToString::to_string)
            .ok_or_else(|| ToolExecutionError::new("no page loaded; call navigate first"))?;

        // Nothing is open in the browser yet, so there is no live document to ask. Report what
        // the last fetch saw and say so: `ready_state` is unknown, not empty.
        let mut reply = json!({
            "success": true,
            "url": url,
            "title": browser.current_title(),
            "ready_state": null,
            "live": false
        });
        if all_tabs {
            reply["tabs"] = json!([]);
            reply["tab_count"] = json!(0);
        }
        return Ok(ToolEffect::reply_json(&reply));
    }

    let mut bridge = browser
        .acquire_bridge()
        .await
        .map_err(|e| ToolExecutionError::new(e.to_string()))?;

    let live_url_result = bridge.evaluate("window.location.href").await;
    let title_result = bridge.evaluate("document.title").await;
    let ready_state_result = bridge.evaluate("document.readyState").await;
    let tabs_result = if all_tabs {
        Some(bridge.list_tabs().await)
    } else {
        None
    };

    drop(bridge);

    let live_url = eval_string(live_url_result);
    let title = eval_string(title_result);
    let ready_state = eval_string(ready_state_result);

    let url = match &live_url {
        Ok(Some(url)) => url.clone(),
        _ => browser
            .current_url()
            .map(ToString::to_string)
            .ok_or_else(|| ToolExecutionError::new("no page loaded; call navigate first"))?,
    };

    let mut reply = json!({
        "success": true,
        "url": url,
        "title": title.clone().ok().flatten().unwrap_or_default(),
        "ready_state": ready_state.clone().ok().flatten().unwrap_or_default(),
        "live": true
    });
    for (key, outcome) in [
        ("title_error", &title),
        ("ready_state_error", &ready_state),
        ("live_url_error", &live_url),
    ] {
        if let Err(message) = outcome {
            reply[key] = Value::String(message.clone());
        }
    }

    match tabs_result {
        Some(Ok(raw)) => {
            let tabs = tabs_json(&raw, browser.page_index());
            reply["tab_count"] = json!(tabs.len());
            reply["tabs"] = Value::Array(tabs);
        }
        Some(Err(e)) => reply["tabs_error"] = Value::String(e.to_string()),
        None => {}
    }

    Ok(ToolEffect::reply_json(&reply))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{clip_title, execute, tabs_json, MAX_TAB_TITLE_CHARS};
    use crate::state::CrawlState;
    use crate::tools::test_support::browser_with_observations;
    use crate::ToolEffect;

    #[tokio::test]
    async fn errors_when_no_page_is_loaded() {
        let mut browser = browser_with_observations(vec![]);
        let mut state = CrawlState::default();

        let err = execute(&json!({}), &mut browser, &mut state)
            .await
            .expect_err("no page yet");

        assert!(err.to_string().contains("no page loaded"), "{err}");
    }

    #[tokio::test]
    async fn without_a_browser_page_reports_the_fetched_title_and_unknown_ready_state() {
        let mut browser = browser_with_observations(vec![]);
        browser.set_navigated_url("https://example.com/", false);
        browser.set_current_title(Some("Example Domain"));
        let mut state = CrawlState::default();

        let effect = execute(&json!({}), &mut browser, &mut state)
            .await
            .expect("reply");
        let ToolEffect::Reply(body) = effect else {
            panic!("expected Reply");
        };
        let reply: serde_json::Value = serde_json::from_str(&body).expect("json");

        assert_eq!(reply["url"], "https://example.com/");
        assert_eq!(reply["title"], "Example Domain");
        assert!(reply["ready_state"].is_null(), "{reply}");
        assert_eq!(reply["live"], false);
        assert!(
            reply.get("seq").is_none(),
            "a read-only call has no seq: {reply}"
        );
    }

    #[test]
    fn tabs_json_marks_only_the_agents_tab_as_active() {
        let raw = json!({"tabs": [
            {"index": 0, "url": "https://a.test/", "title": "A"},
            {"index": 2, "url": "https://b.test/", "title": "B"},
        ]});

        let tabs = tabs_json(&raw, 2);

        assert_eq!(tabs.len(), 2);
        assert_eq!(tabs[0]["active"], false);
        assert_eq!(tabs[1]["active"], true);
        assert_eq!(tabs[1]["index"], 2, "sparse indices are kept as reported");
    }

    #[test]
    fn tabs_json_tolerates_missing_fields_and_garbage() {
        assert!(tabs_json(&json!({}), 0).is_empty());
        let tabs = tabs_json(&json!({"tabs": [{"index": 1}, {"url": "no index"}]}), 0);
        assert_eq!(tabs.len(), 1, "an entry without an index is dropped");
        assert_eq!(tabs[0]["url"], "");
        assert_eq!(tabs[0]["title"], "");
    }

    #[test]
    fn long_titles_are_clipped_to_a_glance() {
        let long = "x".repeat(MAX_TAB_TITLE_CHARS * 3);
        let clipped = clip_title(&long);
        assert_eq!(clipped.chars().count(), MAX_TAB_TITLE_CHARS);
        assert!(clipped.ends_with('\u{2026}'));
        assert_eq!(clip_title("short"), "short");
    }

    #[tokio::test]
    async fn all_tabs_without_a_browser_page_lists_nothing() {
        let mut browser = browser_with_observations(vec![]);
        browser.set_navigated_url("https://example.com/", false);
        let mut state = CrawlState::default();

        let effect = execute(&json!({"all_tabs": true}), &mut browser, &mut state)
            .await
            .expect("reply");
        let ToolEffect::Reply(body) = effect else {
            panic!("expected Reply");
        };
        let reply: serde_json::Value = serde_json::from_str(&body).expect("json");

        assert_eq!(reply["tabs"], json!([]));
        assert_eq!(reply["tab_count"], 0);
    }
}
