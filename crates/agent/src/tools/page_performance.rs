use serde_json::{json, Value};

use crate::BrowserContext;
use crate::{CrawlError, ToolEffect, ToolExecutionError};

pub fn parse_input(_input: &Value) -> Result<(), CrawlError> {
    // No parameters required
    Ok(())
}

pub async fn execute(
    _input: &Value,
    browser: &mut BrowserContext,
) -> Result<ToolEffect, ToolExecutionError> {
    // Get navigation timing
    let nav_script = r#"
JSON.stringify(performance.getEntriesByType('navigation').map(function(e) {
  return {
    ttfb_ms: Math.round(e.responseStart - e.requestStart),
    dom_interactive_ms: Math.round(e.domInteractive),
    dom_complete_ms: Math.round(e.domComplete),
    load_event_ms: Math.round(e.loadEventEnd),
    transfer_size_bytes: e.transferSize || 0,
    decoded_size_bytes: e.decodedBodySize || 0
  };
})[0] || null)
"#;

    let nav_result = browser
        .acquire_bridge()
        .await
        .map_err(|e| ToolExecutionError::new(e.to_string()))?
        .evaluate(nav_script)
        .await
        .map_err(|e| ToolExecutionError::new(e.to_string()))?;

    let navigation = nav_result.get("value").cloned().unwrap_or(Value::Null);

    // Get resource timing
    let res_script = r#"
JSON.stringify(performance.getEntriesByType('resource').map(function(e) {
  return {
    url: e.name,
    type: e.initiatorType,
    transfer_size_kb: Math.round(e.transferSize / 1024 * 10) / 10,
    decoded_size_kb: Math.round(e.decodedBodySize / 1024 * 10) / 10,
    duration_ms: Math.round(e.duration)
  };
}).sort(function(a, b) { return b.transfer_size_kb - a.transfer_size_kb; }).slice(0, 20))
"#;

    let res_result = browser
        .acquire_bridge()
        .await
        .map_err(|e| ToolExecutionError::new(e.to_string()))?
        .evaluate(res_script)
        .await
        .map_err(|e| ToolExecutionError::new(e.to_string()))?;

    let resources = res_result.get("value").cloned().unwrap_or(Value::Array(vec![]));

    // Build summary
    let mut total_requests = 0;
    let mut total_transfer_kb = 0.0;
    let mut largest: Option<(String, f64)> = None;
    let mut slowest: Option<(String, i64)> = None;

    if let Value::Array(ref res_array) = resources {
        total_requests = res_array.len();

        for res in res_array {
            if let Some(size_kb) = res.get("transfer_size_kb").and_then(|v| v.as_f64()) {
                total_transfer_kb += size_kb;

                if largest.is_none() || size_kb > largest.as_ref().unwrap().1 {
                    if let Some(url) = res.get("url").and_then(|v| v.as_str()) {
                        largest = Some((url.to_string(), size_kb));
                    }
                }
            }

            if let Some(duration_ms) = res.get("duration_ms").and_then(|v| v.as_i64()) {
                if slowest.is_none() || duration_ms > slowest.as_ref().unwrap().1 {
                    if let Some(url) = res.get("url").and_then(|v| v.as_str()) {
                        slowest = Some((url.to_string(), duration_ms));
                    }
                }
            }
        }
    }

    let summary = json!({
        "total_requests": total_requests,
        "total_transfer_kb": (total_transfer_kb * 10.0).round() / 10.0,
        "largest": if let Some((url, size)) = largest {
            json!({"url": url, "size_kb": size})
        } else {
            Value::Null
        },
        "slowest": if let Some((url, duration)) = slowest {
            json!({"url": url, "duration_ms": duration})
        } else {
            Value::Null
        }
    });

    Ok(ToolEffect::reply_json(&json!({
        "success": true,
        "navigation": navigation,
        "resources": resources,
        "summary": summary
    })))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn parses_input_no_params() {
        let input = json!({});
        assert!(parse_input(&input).is_ok());
    }

    #[test]
    fn parses_input_with_extra_fields() {
        let input = json!({"extra": "field"});
        assert!(parse_input(&input).is_ok());
    }
}
