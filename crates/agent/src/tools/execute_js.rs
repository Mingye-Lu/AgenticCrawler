use serde_json::{json, Value};

use crate::state::CrawlState;
use crate::BrowserContext;
use crate::{CrawlError, ToolEffect, ToolExecutionError};

const MAX_SETTLE_MS: u64 = 5_000;

pub fn parse_input(input: &Value) -> Result<(String, u64), CrawlError> {
    let script = input
        .get("script")
        .and_then(|v| v.as_str())
        .map(String::from)
        .ok_or_else(|| CrawlError::new("execute_js requires 'script' field"))?;

    let settle_ms = match input.get("settle_ms") {
        None => 0,
        Some(v) => {
            let settle_ms = v.as_u64().ok_or_else(|| {
                CrawlError::new("execute_js settle_ms must be a non-negative integer")
            })?;
            if settle_ms > MAX_SETTLE_MS {
                return Err(CrawlError::new(format!(
                    "execute_js settle_ms must be <= {MAX_SETTLE_MS}"
                )));
            }
            settle_ms
        }
    };

    Ok((script, settle_ms))
}

pub async fn execute(
    input: &Value,
    browser: &mut BrowserContext,
    crawl_state: &CrawlState,
) -> Result<ToolEffect, ToolExecutionError> {
    let (script, settle_ms) = parse_input(input)?;

    let mut bridge = browser
        .acquire_bridge()
        .await
        .map_err(|e| ToolExecutionError::new(e.to_string()))?;

    // Evaluate the caller's script unmodified so multi-statement scripts keep
    // the completion-value (last-expression) semantics the bridge already
    // provides. The settle delay is a separate round-trip rather than being
    // spliced into the script text, so it can't turn a valid script into
    // invalid JS (e.g. wrapping a statement list in a `return (...)` expression).
    let result = bridge
        .evaluate(&script)
        .await
        .map_err(|e| ToolExecutionError::new(e.to_string()))?;

    if settle_ms > 0 {
        bridge
            .evaluate(&format!(
                "await new Promise(r => setTimeout(r, {settle_ms}))"
            ))
            .await
            .map_err(|e| ToolExecutionError::new(e.to_string()))?;
    }

    drop(bridge);

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

    #[test]
    fn rejects_negative_settle_ms() {
        let input = json!({"script": "document.title", "settle_ms": -50});
        let err = parse_input(&input).unwrap_err();
        assert!(err.to_string().contains("non-negative"));
    }

    #[test]
    fn rejects_settle_ms_above_max() {
        let input = json!({"script": "document.title", "settle_ms": MAX_SETTLE_MS + 1});
        let err = parse_input(&input).unwrap_err();
        assert!(err.to_string().contains(&MAX_SETTLE_MS.to_string()));
    }

    #[test]
    fn allows_settle_ms_at_max() {
        let input = json!({"script": "document.title", "settle_ms": MAX_SETTLE_MS});
        let (_, settle_ms) = parse_input(&input).unwrap();
        assert_eq!(settle_ms, MAX_SETTLE_MS);
    }

    #[tokio::test]
    async fn evaluates_multi_statement_script_unmodified_with_settle() {
        use crate::tools::test_support::{
            browser_with_evaluate_recorder, take_recorded_evaluate_scripts,
        };

        let (mut browser, sink) = browser_with_evaluate_recorder();
        let crawl_state = CrawlState::default();

        let script = "document.querySelector('.toggle').click(); document.querySelector('.toggle').getAttribute('aria-checked')";
        let input = json!({"script": script, "settle_ms": 50});

        execute(&input, &mut browser, &crawl_state)
            .await
            .expect("execute should succeed");

        let calls = take_recorded_evaluate_scripts(&sink).await;
        assert_eq!(
            calls[0], script,
            "the caller's script must be evaluated byte-for-byte, not spliced into a wrapper expression"
        );
        assert_eq!(
            calls.len(),
            2,
            "settle delay must be a separate evaluate call"
        );
        assert!(calls[1].contains("setTimeout"));
        assert!(calls[1].contains("50"));
    }

    #[tokio::test]
    async fn skips_settle_call_when_settle_ms_is_zero() {
        use crate::tools::test_support::{
            browser_with_evaluate_recorder, take_recorded_evaluate_scripts,
        };

        let (mut browser, sink) = browser_with_evaluate_recorder();
        let crawl_state = CrawlState::default();

        let input = json!({"script": "document.title"});
        execute(&input, &mut browser, &crawl_state)
            .await
            .expect("execute should succeed");

        let calls = take_recorded_evaluate_scripts(&sink).await;
        assert_eq!(calls, vec!["document.title".to_string()]);
    }
}
