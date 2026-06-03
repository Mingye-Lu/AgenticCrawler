use serde_json::Value;

use crate::BrowserContext;
use crate::{CrawlError, ToolEffect, ToolExecutionError};

#[derive(Debug)]
struct ClickAtInput {
    x: f64,
    y: f64,
}

fn parse_input(input: &Value) -> Result<ClickAtInput, CrawlError> {
    let x = input
        .get("x")
        .and_then(Value::as_f64)
        .ok_or_else(|| CrawlError::new("missing required field: x"))?;

    let y = input
        .get("y")
        .and_then(Value::as_f64)
        .ok_or_else(|| CrawlError::new("missing required field: y"))?;

    Ok(ClickAtInput { x, y })
}

pub async fn execute(
    input: &Value,
    browser: &mut BrowserContext,
) -> Result<ToolEffect, ToolExecutionError> {
    let params = parse_input(input)?;
    browser
        .acquire_bridge()
        .await
        .map_err(|e| ToolExecutionError::new(e.to_string()))?
        .click_at(params.x, params.y)
        .await
        .map_err(|e| ToolExecutionError::new(e.to_string()))?;

    let page_state = super::feedback::post_action_page_state(browser).await;

    Ok(ToolEffect::reply_json(&serde_json::json!({
        "success": true,
        "message": format!("Clicked at coordinates ({}, {})", params.x, params.y),
        "page_state": page_state
    })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_valid_coordinates() {
        let input = json!({"x": 100.0, "y": 200.0});
        let result = parse_input(&input).unwrap();
        assert!((result.x - 100.0).abs() < f64::EPSILON);
        assert!((result.y - 200.0).abs() < f64::EPSILON);
    }

    #[test]
    fn parse_integer_coordinates() {
        let input = json!({"x": 150, "y": 300});
        let result = parse_input(&input).unwrap();
        assert!((result.x - 150.0).abs() < f64::EPSILON);
        assert!((result.y - 300.0).abs() < f64::EPSILON);
    }

    #[test]
    fn parse_missing_x_returns_error() {
        let input = json!({"y": 200});
        let err = parse_input(&input).unwrap_err();
        assert!(err.to_string().contains('x'));
    }

    #[test]
    fn parse_missing_y_returns_error() {
        let input = json!({"x": 100});
        let err = parse_input(&input).unwrap_err();
        assert!(err.to_string().contains('y'));
    }

    #[test]
    fn parse_non_numeric_returns_error() {
        let input = json!({"x": "abc", "y": 200});
        assert!(parse_input(&input).is_err());
    }

    #[test]
    fn click_at_response_includes_page_state() {
        let mock_pm = json!({
            "headings": [], "landmarks": [], "forms": [], "links": [],
            "interactive": {}, "meta": {"title": "Test", "url": "https://test.com", "description": ""}
        });
        let page_state = crate::tools::feedback::build_page_state_from_map(mock_pm);
        let response = json!({
            "success": true,
            "message": "Clicked at coordinates (100, 200)",
            "page_state": page_state
        });
        assert!(response["page_state"]["url"].is_string());
        assert!(response["page_state"]["title"].is_string());
        assert!(!response["page_state"]["page_map"].is_null());
    }
}
