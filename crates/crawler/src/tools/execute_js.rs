use serde_json::{json, Value};

use crate::CrawlError;

pub fn parse_input(input: &Value) -> Result<String, CrawlError> {
    input
        .get("script")
        .and_then(|v| v.as_str())
        .map(String::from)
        .ok_or_else(|| CrawlError::new("execute_js requires 'script' field"))
}

pub fn execute(input: &Value) -> Result<Value, CrawlError> {
    let script = parse_input(input)?;
    Ok(json!({
        "tool": "execute_js",
        "script": script,
        "result": null,
        "note": "bridge call required at runtime"
    }))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn parses_script() {
        let input = json!({"script": "document.title"});
        let script = parse_input(&input).unwrap();
        assert_eq!(script, "document.title");
    }

    #[test]
    fn fails_without_script() {
        let input = json!({});
        assert!(parse_input(&input).is_err());
    }

    #[test]
    fn fails_with_non_string_script() {
        let input = json!({"script": 42});
        assert!(parse_input(&input).is_err());
    }

    #[test]
    fn execute_returns_tool_name() {
        let input = json!({"script": "return 1+1"});
        let result = execute(&input).unwrap();
        assert_eq!(result["tool"], "execute_js");
        assert_eq!(result["script"], "return 1+1");
    }
}
