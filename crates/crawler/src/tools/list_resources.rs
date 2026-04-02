use serde_json::{json, Value};

use crate::CrawlError;

pub fn parse_input(input: &Value) -> Result<(Option<String>, Option<String>), CrawlError> {
    let type_pattern = input
        .get("type_pattern")
        .and_then(|v| v.as_str())
        .map(String::from);
    let name_pattern = input
        .get("name_pattern")
        .and_then(|v| v.as_str())
        .map(String::from);
    Ok((type_pattern, name_pattern))
}

pub fn execute(input: &Value) -> Result<Value, CrawlError> {
    let (_type_pattern, _name_pattern) = parse_input(input)?;
    Ok(json!({
        "tool": "list_resources",
        "links": [],
        "images": [],
        "forms": [],
        "note": "bridge call required at runtime"
    }))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn parses_empty_input() {
        let input = json!({});
        let (tp, np) = parse_input(&input).unwrap();
        assert!(tp.is_none());
        assert!(np.is_none());
    }

    #[test]
    fn parses_type_pattern() {
        let input = json!({"type_pattern": "image"});
        let (tp, _np) = parse_input(&input).unwrap();
        assert_eq!(tp.as_deref(), Some("image"));
    }

    #[test]
    fn parses_name_pattern() {
        let input = json!({"name_pattern": "logo"});
        let (_tp, np) = parse_input(&input).unwrap();
        assert_eq!(np.as_deref(), Some("logo"));
    }

    #[test]
    fn execute_returns_empty_resource_lists() {
        let input = json!({});
        let result = execute(&input).unwrap();
        assert!(result["links"].is_array());
        assert!(result["images"].is_array());
        assert!(result["forms"].is_array());
    }
}
