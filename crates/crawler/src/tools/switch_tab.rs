use serde_json::{json, Value};

use crate::CrawlError;

pub fn parse_input(input: &Value) -> Result<i64, CrawlError> {
    input
        .get("tab_index")
        .or_else(|| input.get("index"))
        .and_then(serde_json::Value::as_i64)
        .ok_or_else(|| CrawlError::new("switch_tab requires 'tab_index' or 'index' field"))
}

pub fn execute(input: &Value) -> Result<Value, CrawlError> {
    let _index = parse_input(input)?;
    Ok(json!({
        "tool": "switch_tab",
        "success": false,
        "error": "multi-tab not supported in single-page spike"
    }))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn parses_tab_index() {
        let input = json!({"tab_index": 2});
        let idx = parse_input(&input).unwrap();
        assert_eq!(idx, 2);
    }

    #[test]
    fn parses_index_alias() {
        let input = json!({"index": 0});
        let idx = parse_input(&input).unwrap();
        assert_eq!(idx, 0);
    }

    #[test]
    fn fails_without_index() {
        let input = json!({});
        assert!(parse_input(&input).is_err());
    }

    #[test]
    fn execute_returns_unsupported() {
        let input = json!({"tab_index": 1});
        let result = execute(&input).unwrap();
        assert_eq!(result["success"], false);
        assert!(result["error"].as_str().unwrap().contains("not supported"));
    }
}
