use serde_json::{json, Value};

use crate::browser::BrowserContext;
use crate::CrawlError;

pub fn parse_input(input: &Value) -> Result<String, CrawlError> {
    input
        .get("script")
        .and_then(|v| v.as_str())
        .map(String::from)
        .ok_or_else(|| CrawlError::new("execute_js requires 'script' field"))
}

pub async fn execute(input: &Value, browser: &mut BrowserContext) -> Result<Value, CrawlError> {
    let script = parse_input(input)?;

    let result = browser
        .bridge_mut()
        .evaluate(&script)
        .await
        .map_err(|e| CrawlError::new(e.to_string()))?;

    let value = result.get("value").cloned().unwrap_or(Value::Null);

    Ok(json!({
        "success": true,
        "result": value
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
}
