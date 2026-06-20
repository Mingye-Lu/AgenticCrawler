use serde_json::{json, Value};

use crate::{BrowserContext, ToolEffect, ToolExecutionError};

/// axe-core 4.10.3 minified JS for WCAG accessibility auditing.
static AXE_CORE_JS: &str = include_str!("../../assets/axe.min.js");

/// Valid WCAG standards that axe-core supports as tag values.
const VALID_STANDARDS: &[&str] = &["wcag2a", "wcag2aa", "wcag21aa", "wcag22aa"];

/// Valid impact levels for filtering violations.
const VALID_IMPACTS: &[&str] = &["critical", "serious", "moderate", "minor", "all"];

#[allow(clippy::too_many_lines)]
pub async fn execute(
    input: &Value,
    browser: &mut BrowserContext,
) -> Result<ToolEffect, ToolExecutionError> {
    let scope = input.get("scope").and_then(Value::as_str);
    let standard = input
        .get("standard")
        .and_then(Value::as_str)
        .unwrap_or("wcag2aa");
    let impact_filter = input.get("impact").and_then(Value::as_str).unwrap_or("all");

    if !VALID_STANDARDS.contains(&standard) {
        return Err(ToolExecutionError::new(format!(
            "invalid standard '{standard}'. Must be one of: {}",
            VALID_STANDARDS.join(", ")
        )));
    }

    if !VALID_IMPACTS.contains(&impact_filter) {
        return Err(ToolExecutionError::new(format!(
            "invalid impact '{impact_filter}'. Must be one of: {}",
            VALID_IMPACTS.join(", ")
        )));
    }

    let mut bridge = browser
        .acquire_bridge()
        .await
        .map_err(|e| ToolExecutionError::new(e.to_string()))?;

    let inject_script =
        format!("if (!window.axe) {{ {AXE_CORE_JS} }}; typeof window.axe !== 'undefined'");
    let inject_result = bridge
        .evaluate(&inject_script)
        .await
        .map_err(|e| ToolExecutionError::new(format!("failed to inject axe-core: {e}")))?;

    let injected = inject_result
        .get("value")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if !injected {
        return Err(ToolExecutionError::new(
            "axe-core injection failed: window.axe is not available after injection".to_string(),
        ));
    }

    let context_js = match scope {
        Some(sel) => serde_json::to_string(sel)
            .map_err(|e| ToolExecutionError::new(format!("invalid scope selector: {e}")))?,
        None => "document".to_string(),
    };

    let run_script = format!(
        r"(async () => {{
            try {{
                const result = await window.axe.run({context_js}, {{
                    runOnly: {{ type: 'tag', values: ['{standard}'] }},
                    resultTypes: ['violations', 'passes']
                }});
                return JSON.stringify({{
                    violations: result.violations,
                    passes_count: result.passes ? result.passes.length : 0
                }});
            }} catch (e) {{
                return JSON.stringify({{ error: e.message }});
            }}
        }})()"
    );

    let result = bridge
        .evaluate(&run_script)
        .await
        .map_err(|e| ToolExecutionError::new(format!("axe.run() failed: {e}")))?;

    let result_str = result.get("value").and_then(Value::as_str).unwrap_or("{}");

    let axe_result: Value = serde_json::from_str(result_str)
        .map_err(|e| ToolExecutionError::new(format!("failed to parse axe-core result: {e}")))?;

    if let Some(error) = axe_result.get("error").and_then(Value::as_str) {
        return Err(ToolExecutionError::new(format!("axe-core error: {error}")));
    }

    let empty_arr = Vec::new();
    let raw_violations = axe_result["violations"].as_array().unwrap_or(&empty_arr);

    let violations: Vec<Value> = raw_violations
        .iter()
        .filter(|v| impact_filter == "all" || v["impact"].as_str() == Some(impact_filter))
        .map(format_violation)
        .collect();

    let passes_count = axe_result["passes_count"].as_u64().unwrap_or(0);

    let critical = violations
        .iter()
        .filter(|v| v["impact"] == "critical")
        .count();
    let serious = violations
        .iter()
        .filter(|v| v["impact"] == "serious")
        .count();
    let moderate = violations
        .iter()
        .filter(|v| v["impact"] == "moderate")
        .count();
    let minor = violations.iter().filter(|v| v["impact"] == "minor").count();

    let output = json!({
        "violations": violations,
        "summary": {
            "total_violations": violations.len(),
            "critical": critical,
            "serious": serious,
            "moderate": moderate,
            "minor": minor,
            "passes": passes_count
        }
    });

    Ok(ToolEffect::reply_json(&output))
}

fn format_violation(v: &Value) -> Value {
    let empty_nodes = Vec::new();
    let nodes = v["nodes"].as_array().unwrap_or(&empty_nodes);
    let elements: Vec<Value> = nodes
        .iter()
        .map(|n| {
            let selector = n["target"]
                .as_array()
                .and_then(|t| t.first())
                .and_then(Value::as_str)
                .unwrap_or("");
            json!({
                "selector": selector,
                "html_snippet": n["html"]
            })
        })
        .collect();

    json!({
        "rule_id": v["id"],
        "impact": v["impact"],
        "description": v["description"],
        "help_url": v["helpUrl"],
        "elements": elements
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn axe_core_js_is_loaded() {
        assert!(
            AXE_CORE_JS.len() > 100_000,
            "axe-core JS should be >100KB, got {} bytes",
            AXE_CORE_JS.len()
        );
        assert!(
            AXE_CORE_JS.contains("axe"),
            "axe-core JS should contain 'axe'"
        );
    }

    #[test]
    fn valid_standards_list() {
        assert!(VALID_STANDARDS.contains(&"wcag2a"));
        assert!(VALID_STANDARDS.contains(&"wcag2aa"));
        assert!(VALID_STANDARDS.contains(&"wcag21aa"));
        assert!(VALID_STANDARDS.contains(&"wcag22aa"));
    }

    #[test]
    fn valid_impacts_list() {
        assert!(VALID_IMPACTS.contains(&"critical"));
        assert!(VALID_IMPACTS.contains(&"serious"));
        assert!(VALID_IMPACTS.contains(&"moderate"));
        assert!(VALID_IMPACTS.contains(&"minor"));
        assert!(VALID_IMPACTS.contains(&"all"));
    }
}
