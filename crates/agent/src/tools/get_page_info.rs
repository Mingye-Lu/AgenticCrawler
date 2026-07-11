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

        return Ok(ToolEffect::reply_json(&serde_json::json!({
            "seq": 0,
            "success": true,
            "url": url,
            "title": "",
            "ready_state": ""
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
        "seq": 0,
        "success": true,
        "url": url,
        "title": title,
        "ready_state": ready_state
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

    #[test]
    fn empty_input_is_valid_json_object() {
        let input = json!({});
        assert!(input.is_object());
    }

    #[test]
    fn null_input_is_accepted() {
        let input = serde_json::Value::Null;
        assert!(input.is_null());
    }
}
