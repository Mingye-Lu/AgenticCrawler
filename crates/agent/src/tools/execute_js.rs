use serde_json::{json, Value};

use crate::state::CrawlState;
use crate::BrowserContext;
use crate::{CrawlError, ToolEffect, ToolExecutionError};

pub fn parse_input(input: &Value) -> Result<(String, u64), CrawlError> {
    let script = input
        .get("script")
        .and_then(|v| v.as_str())
        .map(String::from)
        .ok_or_else(|| CrawlError::new("execute_js requires 'script' field"))?;

    let settle_ms = input
        .get("settle_ms")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    Ok((script, settle_ms))
}

pub async fn execute(
    input: &Value,
    browser: &mut BrowserContext,
    crawl_state: &CrawlState,
) -> Result<ToolEffect, ToolExecutionError> {
    let (script, settle_ms) = parse_input(input)?;

    let script_to_eval = if settle_ms > 0 {
        format!(
            "(async () => {{ const __r = await (async () => {{ return ({}); }})(); await new Promise(r => setTimeout(r, {})); return __r; }})()",
            script, settle_ms
        )
    } else {
        script
    };

    let result = browser
        .acquire_bridge()
        .await
        .map_err(|e| ToolExecutionError::new(e.to_string()))?
        .evaluate(&script_to_eval)
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
        let (script, settle_ms) = parse_input(&input).unwrap();
        assert_eq!(script, "document.title");
        assert_eq!(settle_ms, 0);
    }

    #[test]
    fn parses_script_with_settle_ms() {
        let input = json!({"script": "document.title", "settle_ms": 50});
        let (script, settle_ms) = parse_input(&input).unwrap();
        assert_eq!(script, "document.title");
        assert_eq!(settle_ms, 50);
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
