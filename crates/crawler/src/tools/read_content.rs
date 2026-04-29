use serde_json::Value;

use crate::browser::BrowserContext;
use crate::{ToolEffect, ToolError};

#[allow(dead_code)]
const DEFAULT_MAX_CHARS: usize = 10_000;

#[allow(dead_code)]
fn parse_input(input: &Value) -> Result<(Option<String>, Option<String>, usize, usize), ToolError> {
    let heading = input
        .get("heading")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let selector = input
        .get("selector")
        .and_then(Value::as_str)
        .map(str::to_owned);
    if heading.is_none() && selector.is_none() {
        return Err(ToolError(
            "provide at least one of 'heading' or 'selector'".to_string(),
        ));
    }
    #[allow(clippy::cast_possible_truncation)]
    let offset = input
        .get("offset")
        .and_then(Value::as_u64)
        .unwrap_or(0) as usize;
    #[allow(clippy::cast_possible_truncation)]
    let max_chars = input
        .get("max_chars")
        .and_then(Value::as_u64)
        .map_or(DEFAULT_MAX_CHARS, |v| v as usize);
    Ok((heading, selector, offset, max_chars))
}

// Will be wired into ToolRegistry in a follow-up task.
#[allow(dead_code)]
pub async fn execute(input: &Value, browser: &mut BrowserContext) -> Result<ToolEffect, ToolError> {
    let (heading, selector, offset, max_chars) = parse_input(input)?;
    let mut bridge = browser
        .acquire_bridge()
        .await
        .map_err(|e| ToolError(e.to_string()))?;
    let result = bridge
        .read_content(heading.as_deref(), selector.as_deref(), offset, max_chars)
        .await
        .map_err(|e| ToolError(e.to_string()))?;
    Ok(ToolEffect::reply_json(&result))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn parse_input_requires_heading_or_selector() {
        let err = parse_input(&json!({})).unwrap_err();
        assert!(err.0.contains("heading") || err.0.contains("selector"));
    }

    #[test]
    fn parse_input_heading_only_valid() {
        let (heading, selector, offset, max_chars) =
            parse_input(&json!({"heading": "Introduction"})).unwrap();
        assert_eq!(heading.as_deref(), Some("Introduction"));
        assert!(selector.is_none());
        assert_eq!(offset, 0);
        assert_eq!(max_chars, DEFAULT_MAX_CHARS);
    }

    #[test]
    fn parse_input_selector_only_valid() {
        let (heading, selector, offset, max_chars) =
            parse_input(&json!({"selector": "p.intro"})).unwrap();
        assert!(heading.is_none());
        assert_eq!(selector.as_deref(), Some("p.intro"));
        assert_eq!(offset, 0);
        assert_eq!(max_chars, DEFAULT_MAX_CHARS);
    }

    #[test]
    fn parse_input_offset_and_max_chars_defaults() {
        let (_h, _s, offset, max_chars) = parse_input(&json!({"heading": "x"})).unwrap();
        assert_eq!(offset, 0);
        assert_eq!(max_chars, 10_000);
    }

    #[test]
    fn parse_input_offset_and_max_chars_custom() {
        let (_h, _s, offset, max_chars) =
            parse_input(&json!({"heading": "x", "offset": 500, "max_chars": 2000})).unwrap();
        assert_eq!(offset, 500);
        assert_eq!(max_chars, 2000);
    }
}
