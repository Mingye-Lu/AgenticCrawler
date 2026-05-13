use serde_json::{json, Value};

use crate::browser::BrowserContext;
use crate::{ToolEffect, ToolError};

pub async fn execute(input: &Value, browser: &mut BrowserContext) -> Result<ToolEffect, ToolError> {
    let _ = input;

    let result = browser
        .acquire_bridge()
        .await
        .map_err(|e| ToolError(e.to_string()))?
        .list_resources()
        .await
        .map_err(|e| ToolError(e.to_string()))?;

    let links = result.get("links").cloned().unwrap_or_else(|| json!([]));
    let images = result.get("images").cloned().unwrap_or_else(|| json!([]));
    let forms = result.get("forms").cloned().unwrap_or_else(|| json!([]));

    Ok(ToolEffect::reply_json(&json!({
        "links": links,
        "images": images,
        "forms": forms
    })))
}

#[cfg(test)]
mod tests {
    #[test]
    fn list_resources_schema_has_no_filter_params() {
        let specs = crate::mvp_tool_specs();
        let spec = specs.iter().find(|s| s.name == "list_resources").unwrap();
        let schema_str = spec.input_schema.to_string();
        assert!(
            !schema_str.contains("type_pattern"),
            "type_pattern should be removed from schema"
        );
        assert!(
            !schema_str.contains("name_pattern"),
            "name_pattern should be removed from schema"
        );
    }
}
