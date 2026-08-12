use serde_json::{json, Value};

use crate::state::CrawlState;
use crate::BrowserContext;
use crate::{CrawlError, ToolEffect, ToolExecutionError};

use super::feedback::InteractionKind;

pub fn parse_input(input: &Value) -> Result<(String, i64), CrawlError> {
    let direction = input
        .get("direction")
        .and_then(|v| v.as_str())
        .unwrap_or("down")
        .to_string();

    if direction != "up" && direction != "down" {
        return Err(CrawlError::new(format!(
            "invalid scroll direction: {direction}, expected 'up' or 'down'"
        )));
    }

    let pixels = input
        .get("pixels")
        .or_else(|| input.get("amount"))
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(500);

    Ok((direction, pixels))
}

pub async fn execute(
    input: &Value,
    browser: &mut BrowserContext,
    crawl_state: &mut CrawlState,
) -> Result<ToolEffect, ToolExecutionError> {
    let (direction, pixels) = parse_input(input)?;
    let widen = input
        .get("widen")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);

    browser
        .acquire_bridge()
        .await
        .map_err(|e| ToolExecutionError::new(e.to_string()))?
        .scroll(&direction, pixels)
        .await
        .map_err(|e| ToolExecutionError::new(e.to_string()))?;

    let seq = super::seq::increment_seq(crawl_state, browser).await;
    let page_state = super::feedback::post_action_page_state(
        browser,
        crawl_state,
        InteractionKind::Passive,
        None,
        widen,
    )
    .await?;

    Ok(ToolEffect::reply_json(&json!({
        "seq": seq,
        "success": true,
        "message": format!("Scrolled {direction} {pixels}px"),
        "page_state": page_state
    })))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn parses_direction_and_pixels() {
        let input = json!({"direction": "down", "pixels": 300});
        let (dir, px) = parse_input(&input).unwrap();
        assert_eq!(dir, "down");
        assert_eq!(px, 300);
    }

    #[test]
    fn accepts_legacy_amount_key() {
        let input = json!({"direction": "down", "amount": 200});
        let (dir, px) = parse_input(&input).unwrap();
        assert_eq!(dir, "down");
        assert_eq!(px, 200);
    }

    #[test]
    fn defaults_to_down_500() {
        let input = json!({});
        let (dir, px) = parse_input(&input).unwrap();
        assert_eq!(dir, "down");
        assert_eq!(px, 500);
    }

    #[test]
    fn accepts_pixels_key() {
        let input = json!({"direction": "up", "pixels": 200});
        let (dir, px) = parse_input(&input).unwrap();
        assert_eq!(dir, "up");
        assert_eq!(px, 200);
    }

    #[test]
    fn rejects_invalid_direction() {
        let input = json!({"direction": "left"});
        assert!(parse_input(&input).is_err());
    }

    #[test]
    fn scroll_response_includes_page_state() {
        let mock_pm = json!({
            "headings": [], "landmarks": [], "forms": [], "links": [],
            "interactive": {}, "meta": {"title": "Test", "url": "https://test.com", "description": ""}
        });
        let page_state = crate::tools::feedback::build_page_state_from_map(mock_pm);
        let response = json!({
            "success": true,
            "message": "Scrolled down 500px",
            "page_state": page_state
        });
        assert!(response["page_state"]["url"].is_string());
        assert!(response["page_state"]["title"].is_string());
        assert!(!response["page_state"]["page_map"].is_null());
    }
}
