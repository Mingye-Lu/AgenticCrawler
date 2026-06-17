use serde_json::{json, Value};

use crate::state::CrawlState;
use crate::BrowserContext;
use crate::{CrawlError, ToolEffect, ToolExecutionError};

pub struct PressKeyInput {
    pub key: String,
    pub selector: Option<String>,
}

pub fn parse_input(input: &Value) -> Result<PressKeyInput, CrawlError> {
    let key = input
        .get("key")
        .and_then(|v| v.as_str())
        .ok_or_else(|| CrawlError::new("press_key requires 'key' field"))?
        .to_string();

    let selector = input
        .get("selector")
        .and_then(|v| v.as_str())
        .map(String::from);

    Ok(PressKeyInput { key, selector })
}

pub async fn execute(
    input: &Value,
    browser: &mut BrowserContext,
    crawl_state: &CrawlState,
) -> Result<ToolEffect, ToolExecutionError> {
    let parsed = parse_input(input)?;

    let resolved_selector: Option<String> = if let Some(sel) = &parsed.selector {
        let r = super::ref_resolve::resolve_selector(sel, browser.ref_map())
            .map_err(ToolExecutionError::new)?;
        Some(r)
    } else {
        None
    };

    browser
        .acquire_bridge()
        .await
        .map_err(|e| ToolExecutionError::new(e.to_string()))?
        .press_key(&parsed.key, resolved_selector.as_deref())
        .await
        .map_err(|e| ToolExecutionError::new(e.to_string()))?;

    let seq = super::seq::increment_seq(crawl_state, browser).await;
    let page_state = super::feedback::post_action_page_state(browser).await;

    Ok(ToolEffect::reply_json(&json!({
        "seq": seq,
        "success": true,
        "message": format!("Pressed key: {}", parsed.key),
        "page_state": page_state
    })))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn parses_key() {
        let input = json!({"key": "Enter"});
        let parsed = parse_input(&input).unwrap();
        assert_eq!(parsed.key, "Enter");
        assert!(parsed.selector.is_none());
    }

    #[test]
    fn parses_key_with_selector() {
        let input = json!({"key": "Escape", "selector": "#modal"});
        let parsed = parse_input(&input).unwrap();
        assert_eq!(parsed.key, "Escape");
        assert_eq!(parsed.selector.as_deref(), Some("#modal"));
    }

    #[test]
    fn fails_without_key() {
        let input = json!({"selector": "#input"});
        assert!(parse_input(&input).is_err());
    }

    #[test]
    fn press_key_response_includes_page_state() {
        let mock_pm = json!({
            "headings": [], "landmarks": [], "forms": [], "links": [],
            "interactive": {}, "meta": {"title": "Test", "url": "https://test.com", "description": ""}
        });
        let page_state = crate::tools::feedback::build_page_state_from_map(mock_pm);
        let response = json!({
            "success": true,
            "message": "Pressed key: Enter",
            "page_state": page_state
        });
        assert!(response["page_state"]["url"].is_string());
        assert!(response["page_state"]["title"].is_string());
        assert!(!response["page_state"]["page_map"].is_null());
    }
}
