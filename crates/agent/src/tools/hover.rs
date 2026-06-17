use serde_json::{json, Value};

use crate::state::CrawlState;
use crate::BrowserContext;
use crate::{CrawlError, ToolEffect, ToolExecutionError};

pub fn parse_input(input: &Value) -> Result<String, CrawlError> {
    input
        .get("selector")
        .and_then(|v| v.as_str())
        .map(String::from)
        .ok_or_else(|| CrawlError::new("hover requires 'selector' field"))
}

pub async fn execute(
    input: &Value,
    browser: &mut BrowserContext,
    crawl_state: &CrawlState,
) -> Result<ToolEffect, ToolExecutionError> {
    let selector = parse_input(input)?;
    let resolved = super::ref_resolve::resolve_selector(&selector, browser.ref_map())
        .map_err(ToolExecutionError::new)?;

    browser
        .acquire_bridge()
        .await
        .map_err(|e| ToolExecutionError::new(e.to_string()))?
        .hover(&resolved)
        .await
        .map_err(|e| ToolExecutionError::new(e.to_string()))?;

    let seq = super::seq::increment_seq(crawl_state, browser).await;
    let page_state = super::feedback::post_action_page_state(browser).await;

    Ok(ToolEffect::reply_json(&json!({
        "seq": seq,
        "success": true,
        "message": format!("Hovered over: {selector}"),
        "page_state": page_state
    })))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn parses_selector() {
        let input = json!({"selector": ".menu-item"});
        let selector = parse_input(&input).unwrap();
        assert_eq!(selector, ".menu-item");
    }

    #[test]
    fn fails_without_selector() {
        let input = json!({});
        assert!(parse_input(&input).is_err());
    }

    #[test]
    fn fails_with_non_string() {
        let input = json!({"selector": 123});
        assert!(parse_input(&input).is_err());
    }

    #[test]
    fn hover_response_includes_page_state() {
        let mock_pm = json!({
            "headings": [], "landmarks": [], "forms": [], "links": [],
            "interactive": {}, "meta": {"title": "Test", "url": "https://test.com", "description": ""}
        });
        let page_state = crate::tools::feedback::build_page_state_from_map(mock_pm);
        let response = json!({
            "success": true,
            "message": "Hovered over: .menu-item",
            "page_state": page_state
        });
        assert!(response["page_state"]["url"].is_string());
        assert!(response["page_state"]["title"].is_string());
        assert!(!response["page_state"]["page_map"].is_null());
    }
}
