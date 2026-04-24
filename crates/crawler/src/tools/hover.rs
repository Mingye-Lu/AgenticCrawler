use serde_json::{json, Value};

use crate::browser::BrowserContext;
use crate::CrawlError;

pub fn parse_input(input: &Value) -> Result<String, CrawlError> {
    input
        .get("selector")
        .and_then(|v| v.as_str())
        .map(String::from)
        .ok_or_else(|| CrawlError::new("hover requires 'selector' field"))
}

pub async fn execute(input: &Value, browser: &mut BrowserContext) -> Result<Value, CrawlError> {
    let selector = parse_input(input)?;

    browser
        .acquire_bridge()
        .await
        .map_err(|e| CrawlError::new(e.to_string()))?
        .hover(&selector)
        .await
        .map_err(|e| CrawlError::new(e.to_string()))?;

    Ok(json!({
        "success": true,
        "message": format!("Hovered over: {selector}")
    }))
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
}
