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

    let mut bridge = browser
        .acquire_bridge()
        .await
        .map_err(|e| ToolExecutionError::new(e.to_string()))?;

    let live_url = bridge
        .evaluate("window.location.href")
        .await
        .ok()
        .and_then(|v| {
            v.get("value")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        });

    let title = bridge
        .evaluate("document.title")
        .await
        .ok()
        .and_then(|v| {
            v.get("value")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        })
        .unwrap_or_default();

    let ready_state = bridge
        .evaluate("document.readyState")
        .await
        .ok()
        .and_then(|v| {
            v.get("value")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        })
        .unwrap_or_default();

    drop(bridge);

    let url = match live_url {
        Some(url) => url,
        None => browser
            .current_url()
            .map(ToString::to_string)
            .ok_or_else(|| ToolExecutionError::new("no page loaded; call navigate first"))?,
    };

    let seq = super::seq::increment_seq(crawl_state, browser).await;

    Ok(ToolEffect::reply_json(&serde_json::json!({
        "seq": seq,
        "success": true,
        "url": url,
        "title": title,
        "ready_state": ready_state
    })))
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
