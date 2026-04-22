use serde_json::Value;

use crate::browser::BrowserContext;
use crate::CrawlError;

#[derive(Debug)]
struct ClickInput {
    selector: String,
}

fn parse_input(input: &Value) -> Result<ClickInput, CrawlError> {
    let selector = input
        .get("selector")
        .and_then(Value::as_str)
        .ok_or_else(|| CrawlError::new("missing required field: selector"))?;

    if selector.is_empty() {
        return Err(CrawlError::new("selector must not be empty"));
    }

    Ok(ClickInput {
        selector: selector.to_string(),
    })
}

pub async fn execute(input: &Value, browser: &mut BrowserContext) -> Result<Value, CrawlError> {
    let params = parse_input(input)?;
    browser
        .acquire_bridge()
        .await
        .click(&params.selector)
        .await
        .map_err(|e| CrawlError::new(e.to_string()))?;

    Ok(serde_json::json!({
        "success": true,
        "message": format!("Clicked element: {}", params.selector)
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_valid_selector() {
        let input = json!({"selector": ".btn-primary"});
        let result = parse_input(&input).unwrap();
        assert_eq!(result.selector, ".btn-primary");
    }

    #[test]
    fn parse_missing_selector_returns_error() {
        let input = json!({});
        let err = parse_input(&input).unwrap_err();
        assert!(err.to_string().contains("selector"));
    }

    #[test]
    fn parse_empty_selector_returns_error() {
        let input = json!({"selector": ""});
        let err = parse_input(&input).unwrap_err();
        assert!(err.to_string().contains("empty"));
    }

    #[test]
    fn parse_non_string_selector_returns_error() {
        let input = json!({"selector": 123});
        assert!(parse_input(&input).is_err());
    }

    #[test]
    fn parse_complex_selector() {
        let input = json!({"selector": "div.container > ul > li:nth-child(2) a"});
        let result = parse_input(&input).unwrap();
        assert_eq!(result.selector, "div.container > ul > li:nth-child(2) a");
    }
}
