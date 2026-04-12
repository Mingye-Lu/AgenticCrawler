use serde_json::{json, Value};

use crate::browser::BrowserContext;
use crate::CrawlError;

pub fn parse_input(input: &Value) -> Result<i64, CrawlError> {
    Ok(input
        .get("tab_index")
        .or_else(|| input.get("index"))
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(-1))
}

pub async fn execute(input: &Value, browser: &mut BrowserContext) -> Result<Value, CrawlError> {
    let index = parse_input(input)?;

    let result = browser
        .bridge_mut()
        .switch_tab(index)
        .await
        .map_err(|e| CrawlError::new(e.to_string()))?;

    let url = result
        .get("url")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let title = result
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let tab_count = result
        .get("tab_count")
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(0);

    Ok(json!({
        "success": true,
        "url": url,
        "title": title,
        "tab_count": tab_count
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
    fn defaults_to_negative_one() {
        let input = json!({});
        let idx = parse_input(&input).unwrap();
        assert_eq!(idx, -1);
    }
}
