use serde_json::{json, Value};

use crate::browser::BrowserContext;
use crate::CrawlError;

pub fn parse_input(input: &Value) -> (Option<String>, Option<String>) {
    let type_pattern = input
        .get("type_pattern")
        .and_then(|v| v.as_str())
        .map(String::from);
    let name_pattern = input
        .get("name_pattern")
        .and_then(|v| v.as_str())
        .map(String::from);
    (type_pattern, name_pattern)
}

pub async fn execute(input: &Value, browser: &mut BrowserContext) -> Result<Value, CrawlError> {
    let (_type_pattern, _name_pattern) = parse_input(input);

    let result = browser
        .acquire_bridge()
        .await
        .list_resources()
        .await
        .map_err(|e| CrawlError::new(e.to_string()))?;

    let links = result.get("links").cloned().unwrap_or_else(|| json!([]));
    let images = result.get("images").cloned().unwrap_or_else(|| json!([]));
    let forms = result.get("forms").cloned().unwrap_or_else(|| json!([]));

    Ok(json!({
        "links": links,
        "images": images,
        "forms": forms
    }))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn parses_empty_input() {
        let input = json!({});
        let (tp, np) = parse_input(&input);
        assert!(tp.is_none());
        assert!(np.is_none());
    }

    #[test]
    fn parses_type_pattern() {
        let input = json!({"type_pattern": "image"});
        let (tp, _np) = parse_input(&input);
        assert_eq!(tp.as_deref(), Some("image"));
    }

    #[test]
    fn parses_name_pattern() {
        let input = json!({"name_pattern": "logo"});
        let (_tp, np) = parse_input(&input);
        assert_eq!(np.as_deref(), Some("logo"));
    }
}
