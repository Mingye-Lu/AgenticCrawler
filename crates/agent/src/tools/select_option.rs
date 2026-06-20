use serde_json::{json, Value};

use crate::state::CrawlState;
use crate::BrowserContext;
use crate::{CrawlError, ToolEffect, ToolExecutionError};

pub struct SelectOptionInput {
    pub selector: String,
    pub value: String,
}

pub fn parse_input(input: &Value) -> Result<SelectOptionInput, CrawlError> {
    let selector = input
        .get("selector")
        .and_then(|v| v.as_str())
        .ok_or_else(|| CrawlError::new("select_option requires 'selector' field"))?
        .to_string();

    let value = input
        .get("value")
        .or_else(|| input.get("label"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| CrawlError::new("select_option requires 'value' or 'label' field"))?
        .to_string();

    Ok(SelectOptionInput { selector, value })
}

pub async fn execute(
    input: &Value,
    browser: &mut BrowserContext,
    crawl_state: &CrawlState,
) -> Result<ToolEffect, ToolExecutionError> {
    let parsed = parse_input(input)?;
    let resolved = super::ref_resolve::resolve_selector(&parsed.selector, browser.ref_map())
        .map_err(ToolExecutionError::new)?;

    browser
        .acquire_bridge()
        .await
        .map_err(|e| ToolExecutionError::new(e.to_string()))?
        .select_option(&resolved, &parsed.value)
        .await
        .map_err(|e| ToolExecutionError::new(e.to_string()))?;

    let seq = super::seq::increment_seq(crawl_state, browser).await;
    let page_state = super::feedback::post_action_page_state(browser).await;

    Ok(ToolEffect::reply_json(&json!({
        "seq": seq,
        "success": true,
        "message": format!("Selected '{}' in {}", parsed.value, parsed.selector),
        "page_state": page_state
    })))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn parses_selector_and_value() {
        let input = json!({"selector": "#country", "value": "US"});
        let parsed = parse_input(&input).unwrap();
        assert_eq!(parsed.selector, "#country");
        assert_eq!(parsed.value, "US");
    }

    #[test]
    fn accepts_label_as_value() {
        let input = json!({"selector": "select.lang", "label": "English"});
        let parsed = parse_input(&input).unwrap();
        assert_eq!(parsed.value, "English");
    }

    #[test]
    fn fails_without_selector() {
        let input = json!({"value": "US"});
        assert!(parse_input(&input).is_err());
    }

    #[test]
    fn fails_without_value() {
        let input = json!({"selector": "#country"});
        assert!(parse_input(&input).is_err());
    }

    #[test]
    fn select_option_response_includes_page_state() {
        let mock_pm = json!({
            "headings": [], "landmarks": [], "forms": [], "links": [],
            "interactive": {}, "meta": {"title": "Test", "url": "https://test.com", "description": ""}
        });
        let page_state = crate::tools::feedback::build_page_state_from_map(mock_pm);
        let response = json!({
            "success": true,
            "message": "Selected 'US' in #country",
            "page_state": page_state
        });
        assert!(response["page_state"]["url"].is_string());
        assert!(response["page_state"]["title"].is_string());
        assert!(!response["page_state"]["page_map"].is_null());
    }
}
