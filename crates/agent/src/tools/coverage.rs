use serde_json::{json, Value};

use crate::BrowserContext;
use crate::{ToolEffect, ToolExecutionError};

pub async fn execute(
    input: &Value,
    browser: &mut BrowserContext,
) -> Result<ToolEffect, ToolExecutionError> {
    let coverage_type = input.get("type").and_then(Value::as_str).unwrap_or("all");
    let reset = input.get("reset").and_then(Value::as_bool).unwrap_or(false);

    let do_js = coverage_type != "css";
    let do_css = coverage_type != "js";

    let mut bridge = browser
        .acquire_bridge()
        .await
        .map_err(|e| ToolExecutionError::new(e.to_string()))?;

    if reset {
        let _ = bridge.stop_coverage().await;
    }

    let coverage_data = bridge
        .stop_coverage()
        .await
        .unwrap_or_else(|_| browser::CoverageData {
            js_coverage: vec![],
            css_coverage: vec![],
        });

    let _ = bridge.start_coverage(do_js, do_css).await;

    let mut js_output = Vec::new();
    let mut total_unused_js: usize = 0;
    let mut total_unused_css: usize = 0;
    let mut worst_offender_url = String::new();
    let mut worst_offender_pct: f64 = 0.0;

    for entry in &coverage_data.js_coverage {
        let unused_bytes = entry.total_bytes.saturating_sub(entry.used_bytes);
        let unused_pct = if entry.total_bytes > 0 {
            (f64::from(u32::try_from(unused_bytes).unwrap_or(u32::MAX))
                / f64::from(u32::try_from(entry.total_bytes).unwrap_or(u32::MAX)))
                * 100.0
        } else {
            0.0
        };
        let unused_pct = (unused_pct * 10.0).round() / 10.0;

        total_unused_js += unused_bytes;
        if unused_pct > worst_offender_pct {
            worst_offender_pct = unused_pct;
            worst_offender_url.clone_from(&entry.url);
        }

        js_output.push(json!({
            "url": entry.url,
            "total_bytes": entry.total_bytes,
            "used_bytes": entry.used_bytes,
            "unused_bytes": unused_bytes,
            "unused_pct": unused_pct,
        }));
    }

    let mut css_output = Vec::new();
    for entry in &coverage_data.css_coverage {
        let unused_bytes = entry.total_bytes.saturating_sub(entry.used_bytes);
        let unused_pct = if entry.total_bytes > 0 {
            (f64::from(u32::try_from(unused_bytes).unwrap_or(u32::MAX))
                / f64::from(u32::try_from(entry.total_bytes).unwrap_or(u32::MAX)))
                * 100.0
        } else {
            0.0
        };
        let unused_pct = (unused_pct * 10.0).round() / 10.0;

        total_unused_css += unused_bytes;
        if unused_pct > worst_offender_pct {
            worst_offender_pct = unused_pct;
            worst_offender_url.clone_from(&entry.url);
        }

        css_output.push(json!({
            "url": entry.url,
            "total_bytes": entry.total_bytes,
            "used_bytes": entry.used_bytes,
            "unused_bytes": unused_bytes,
            "unused_pct": unused_pct,
        }));
    }

    let total_unused_js_kb = (total_unused_js as f64 / 1024.0 * 10.0).round() / 10.0;
    let total_unused_css_kb = (total_unused_css as f64 / 1024.0 * 10.0).round() / 10.0;

    let result = json!({
        "js_coverage": js_output,
        "css_coverage": css_output,
        "summary": {
            "total_unused_js_kb": total_unused_js_kb,
            "total_unused_css_kb": total_unused_css_kb,
            "worst_offender": if worst_offender_url.is_empty() {
                json!(null)
            } else {
                json!({
                    "url": worst_offender_url,
                    "unused_pct": worst_offender_pct,
                })
            }
        }
    });

    Ok(ToolEffect::reply_json(&result))
}
