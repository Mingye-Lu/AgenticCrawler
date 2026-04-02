use serde_json::{json, Value};

use crate::CrawlError;

pub fn parse_input(input: &Value) -> Result<String, CrawlError> {
    input
        .get("selector")
        .and_then(|v| v.as_str())
        .map(String::from)
        .ok_or_else(|| CrawlError::new("hover requires 'selector' field"))
}

pub fn execute(input: &Value) -> Result<Value, CrawlError> {
    let selector = parse_input(input)?;
    Ok(json!({
        "tool": "hover",
        "selector": selector,
        "success": true,
        "note": "bridge call required at runtime"
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

    #[test]
    fn execute_returns_success() {
        let input = json!({"selector": "#tooltip-trigger"});
        let result = execute(&input).unwrap();
        assert_eq!(result["success"], true);
        assert_eq!(result["selector"], "#tooltip-trigger");
    }
}
