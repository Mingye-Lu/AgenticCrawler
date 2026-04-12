use serde_json::{json, Value};

use crate::browser::BrowserContext;
use crate::CrawlError;

pub struct SelectOptionInput {
    pub selector: String,
    pub value: String,
}

pub fn parse_input(input: &Value) -> Result<SelectOptionInput, CrawlError> {
    let selector = input
        .get("selector")
        .and_then(|v| v.as_str())
        .ok_or_else(|| CrawlError::new("select_option requires 'selector' field"))?
        .to_string();

    let value = input
        .get("value")
        .or_else(|| input.get("label"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| CrawlError::new("select_option requires 'value' or 'label' field"))?
        .to_string();

    Ok(SelectOptionInput { selector, value })
}

pub async fn execute(input: &Value, browser: &mut BrowserContext) -> Result<Value, CrawlError> {
    let parsed = parse_input(input)?;

    browser
        .bridge_mut()
        .select_option(&parsed.selector, &parsed.value)
        .await
        .map_err(|e| CrawlError::new(e.to_string()))?;

    Ok(json!({
        "success": true,
        "message": format!("Selected '{}' in {}", parsed.value, parsed.selector)
    }))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn parses_selector_and_value() {
        let input = json!({"selector": "#country", "value": "US"});
        let parsed = parse_input(&input).unwrap();
        assert_eq!(parsed.selector, "#country");
        assert_eq!(parsed.value, "US");
    }

    #[test]
    fn accepts_label_as_value() {
        let input = json!({"selector": "select.lang", "label": "English"});
        let parsed = parse_input(&input).unwrap();
        assert_eq!(parsed.value, "English");
    }

    #[test]
    fn fails_without_selector() {
        let input = json!({"value": "US"});
        assert!(parse_input(&input).is_err());
    }

    #[test]
    fn fails_without_value() {
        let input = json!({"selector": "#country"});
        assert!(parse_input(&input).is_err());
    }
}
