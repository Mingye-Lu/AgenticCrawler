use std::collections::HashMap;

use serde_json::{json, Value};

use crate::BrowserContext;
use crate::{ToolEffect, ToolExecutionError};

/// Collapse coverage entries that share a URL, keeping the highest `used_bytes`.
///
/// With `resetOnNavigation: false`, a reload makes the same file appear twice —
/// a stale pre-navigation snapshot (typically `used_bytes: 0`) alongside the
/// real post-navigation entry. Keeping the max drops the stale 100%-unused row
/// so it neither shows up nor skews `worst_offender`. First-seen order is kept.
fn dedupe_by_url(entries: &[browser::FileCoverage]) -> Vec<browser::FileCoverage> {
    let mut order: Vec<String> = Vec::new();
    let mut best: HashMap<String, browser::FileCoverage> = HashMap::new();
    for entry in entries {
        if let Some(existing) = best.get_mut(&entry.url) {
            if entry.used_bytes > existing.used_bytes {
                *existing = entry.clone();
            }
        } else {
            order.push(entry.url.clone());
            best.insert(entry.url.clone(), entry.clone());
        }
    }
    order
        .into_iter()
        .filter_map(|url| best.remove(&url))
        .collect()
}

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

    let js_coverage = dedupe_by_url(&coverage_data.js_coverage);
    let css_coverage = dedupe_by_url(&coverage_data.css_coverage);

    let mut js_output = Vec::new();
    let mut total_unused_js: usize = 0;
    let mut total_unused_css: usize = 0;
    let mut worst_offender_url = String::new();
    let mut worst_offender_pct: f64 = 0.0;

    for entry in &js_coverage {
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
    for entry in &css_coverage {
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

    #[allow(clippy::cast_precision_loss)]
    let total_unused_js_kb = (total_unused_js as f64 / 1024.0 * 10.0).round() / 10.0;
    #[allow(clippy::cast_precision_loss)]
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

#[cfg(test)]
mod tests {
    use super::*;
    use browser::FileCoverage;

    fn fc(url: &str, total: usize, used: usize) -> FileCoverage {
        FileCoverage {
            url: url.to_string(),
            total_bytes: total,
            used_bytes: used,
        }
    }

    #[test]
    fn dedupe_by_url_keeps_max_used_and_drops_stale_snapshot() {
        let entries = vec![
            fc("https://x/app.css", 88, 0),
            fc("https://x/app.css", 88, 62),
            fc("https://x/other.css", 50, 10),
        ];
        let deduped = dedupe_by_url(&entries);

        assert_eq!(deduped.len(), 2);
        let app = deduped
            .iter()
            .find(|e| e.url.ends_with("app.css"))
            .expect("app.css survives");
        assert_eq!(app.used_bytes, 62);
        assert_eq!(app.total_bytes, 88);
    }

    #[test]
    fn dedupe_by_url_preserves_first_seen_order() {
        let entries = vec![
            fc("https://x/b.js", 10, 5),
            fc("https://x/a.js", 10, 5),
            fc("https://x/b.js", 10, 9),
        ];
        let deduped = dedupe_by_url(&entries);

        assert_eq!(deduped.len(), 2);
        assert_eq!(deduped[0].url, "https://x/b.js");
        assert_eq!(deduped[0].used_bytes, 9);
        assert_eq!(deduped[1].url, "https://x/a.js");
    }
}
