use serde_json::{json, Value};

use crate::state::CrawlState;
use crate::BrowserContext;
use crate::{CrawlError, ToolEffect, ToolExecutionError};

pub struct ExecuteJsInput {
    pub script: String,
    pub hover_selector: Option<String>,
}

pub fn parse_input(input: &Value) -> Result<ExecuteJsInput, CrawlError> {
    let script = input
        .get("script")
        .and_then(|v| v.as_str())
        .map(String::from)
        .ok_or_else(|| CrawlError::new("execute_js requires 'script' field"))?;

    let hover_selector = input
        .get("hover_selector")
        .and_then(|v| v.as_str())
        .map(String::from);

    Ok(ExecuteJsInput {
        script,
        hover_selector,
    })
}

pub async fn execute(
    input: &Value,
    browser: &mut BrowserContext,
    crawl_state: &CrawlState,
) -> Result<ToolEffect, ToolExecutionError> {
    let params = parse_input(input)?;

    if let Some(hover_selector) = &params.hover_selector {
        browser
            .acquire_bridge()
            .await
            .map_err(|e| ToolExecutionError::new(e.to_string()))?
            .hover(hover_selector)
            .await
            .map_err(|e| {
                ToolExecutionError::new(format!(
                    "execute_js: failed to hover over '{hover_selector}' before evaluating script: {e}"
                ))
            })?;
    }

    let result = browser
        .acquire_bridge()
        .await
        .map_err(|e| ToolExecutionError::new(e.to_string()))?
        .evaluate(&params.script)
        .await
        .map_err(|e| ToolExecutionError::new(e.to_string()))?;

    let seq = super::seq::increment_seq(crawl_state, browser).await;
    let value = result.get("value").cloned().unwrap_or(Value::Null);

    Ok(ToolEffect::reply_json(&json!({
        "seq": seq,
        "success": true,
        "result": value
    })))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn parses_script() {
        let input = json!({"script": "document.title"});
        let parsed = parse_input(&input).unwrap();
        assert_eq!(parsed.script, "document.title");
        assert!(parsed.hover_selector.is_none());
    }

    #[test]
    fn parses_hover_selector() {
        let input = json!({"script": "getComputedStyle(document.querySelector('.btn')).color", "hover_selector": ".btn"});
        let parsed = parse_input(&input).unwrap();
        assert_eq!(parsed.script, "getComputedStyle(document.querySelector('.btn')).color");
        assert_eq!(parsed.hover_selector.as_deref(), Some(".btn"));
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
