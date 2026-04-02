use serde_json::{json, Value};

use crate::CrawlError;

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

pub fn execute(input: &Value) -> Result<Value, CrawlError> {
    let parsed = parse_input(input)?;
    Ok(json!({
        "tool": "press_key",
        "key": parsed.key,
        "selector": parsed.selector,
        "success": true,
        "note": "bridge call required at runtime"
    }))
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
    fn execute_returns_success() {
        let input = json!({"key": "Tab"});
        let result = execute(&input).unwrap();
        assert_eq!(result["success"], true);
        assert_eq!(result["key"], "Tab");
    }
}
