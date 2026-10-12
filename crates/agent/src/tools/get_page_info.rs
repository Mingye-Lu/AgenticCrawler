use serde_json::Value;

use crate::state::CrawlState;
use crate::BrowserContext;
use crate::{ToolEffect, ToolExecutionError};

pub async fn execute(
    input: &Value,
    browser: &mut BrowserContext,
    crawl_state: &mut CrawlState,
) -> Result<ToolEffect, ToolExecutionError> {
    let _ = input;
    let _ = crawl_state;

    if !browser.is_browser_loaded() {
        let url = browser
            .current_url()
            .map(ToString::to_string)
            .ok_or_else(|| ToolExecutionError::new("no page loaded; call navigate first"))?;

        // Nothing is open in the browser yet, so there is no live document to ask. Report what
        // the last fetch saw and say so: `ready_state` is unknown, not empty.
        return Ok(ToolEffect::reply_json(&serde_json::json!({
            "success": true,
            "url": url,
            "title": browser.current_title(),
            "ready_state": null,
            "live": false
        })));
    }

    let mut bridge = browser
        .acquire_bridge()
        .await
        .map_err(|e| ToolExecutionError::new(e.to_string()))?;

    let live_url_result = bridge.evaluate("window.location.href").await;
    let title_result = bridge.evaluate("document.title").await;
    let ready_state_result = bridge.evaluate("document.readyState").await;

    drop(bridge);

    let mut live_url_error = None;
    let live_url = match live_url_result {
        Ok(v) => v
            .get("value")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        Err(e) => {
            live_url_error = Some(e.to_string());
            None
        }
    };

    let mut title_error = None;
    let title = match title_result {
        Ok(v) => v
            .get("value")
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .unwrap_or_default(),
        Err(e) => {
            title_error = Some(e.to_string());
            String::new()
        }
    };

    let mut ready_state_error = None;
    let ready_state = match ready_state_result {
        Ok(v) => v
            .get("value")
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .unwrap_or_default(),
        Err(e) => {
            ready_state_error = Some(e.to_string());
            String::new()
        }
    };

    let url = match live_url {
        Some(url) => url,
        None => browser
            .current_url()
            .map(ToString::to_string)
            .ok_or_else(|| ToolExecutionError::new("no page loaded; call navigate first"))?,
    };

    let mut reply = serde_json::json!({
        "success": true,
        "url": url,
        "title": title,
        "ready_state": ready_state,
        "live": true
    });

    if let Some(err) = title_error {
        reply["title_error"] = serde_json::Value::String(err);
    }
    if let Some(err) = ready_state_error {
        reply["ready_state_error"] = serde_json::Value::String(err);
    }
    if let Some(err) = live_url_error {
        reply["live_url_error"] = serde_json::Value::String(err);
    }

    Ok(ToolEffect::reply_json(&reply))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::execute;
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
}
