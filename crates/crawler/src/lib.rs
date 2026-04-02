use runtime::PermissionMode;
use serde_json::json;

pub mod agent;
pub mod browser;
pub mod output;
pub mod playwright;
pub mod state;
pub mod tool_registry;
pub mod tools;

pub use agent::{AgentHandle, AgentState, CrawlAgent, CrawlError, CrawlResult};
pub use browser::BrowserContext;
pub use output::OutputFormat;
pub use playwright::{PageInfo, PlaywrightBridge, PlaywrightBridgeError};
pub use state::CrawlState;
pub use tool_registry::{ToolHandler, ToolRegistry};

/// Specification for a single tool that the agent can invoke.
pub struct ToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: serde_json::Value,
    pub required_permission: PermissionMode,
}

/// Returns the built-in tool specifications.
#[must_use]
pub fn mvp_tool_specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "navigate",
            description: "Navigate to a URL and get page content",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "url": { "type": "string" }
                },
                "required": ["url"],
                "additionalProperties": false
            }),
            required_permission: PermissionMode::DangerFullAccess,
        },
        ToolSpec {
            name: "click",
            description: "Click on an element by CSS selector",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "selector": { "type": "string" }
                },
                "required": ["selector"],
                "additionalProperties": false
            }),
            required_permission: PermissionMode::DangerFullAccess,
        },
        ToolSpec {
            name: "fill_form",
            description: "Fill a form field with a value",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "fields": {
                        "type": "object",
                        "additionalProperties": { "type": "string" }
                    },
                    "submit": { "type": "boolean" },
                    "form_selector": { "type": "string" }
                },
                "required": ["fields"],
                "additionalProperties": false
            }),
            required_permission: PermissionMode::DangerFullAccess,
        },
        ToolSpec {
            name: "extract_data",
            description: "Extract structured data from the page",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "instruction": { "type": "string" },
                    "data": {}
                },
                "required": ["instruction", "data"],
                "additionalProperties": false
            }),
            required_permission: PermissionMode::ReadOnly,
        },
        ToolSpec {
            name: "screenshot",
            description: "Capture a screenshot of the current page",
            input_schema: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
            required_permission: PermissionMode::DangerFullAccess,
        },
        ToolSpec {
            name: "go_back",
            description: "Navigate back to the previous page",
            input_schema: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
            required_permission: PermissionMode::DangerFullAccess,
        },
        ToolSpec {
            name: "scroll",
            description: "Scroll the page up or down",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "direction": { "type": "string", "enum": ["up", "down"] },
                    "amount": { "type": "integer" }
                },
                "additionalProperties": false
            }),
            required_permission: PermissionMode::DangerFullAccess,
        },
        ToolSpec {
            name: "wait",
            description: "Wait for an element or timeout",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "selector": { "type": "string" },
                    "seconds": { "type": "number" }
                },
                "additionalProperties": false
            }),
            required_permission: PermissionMode::DangerFullAccess,
        },
        ToolSpec {
            name: "select_option",
            description: "Select a dropdown option by value",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "selector": { "type": "string" },
                    "value": { "type": "string" },
                    "label": { "type": "string" },
                    "index": { "type": "integer" }
                },
                "required": ["selector"],
                "additionalProperties": false
            }),
            required_permission: PermissionMode::DangerFullAccess,
        },
        ToolSpec {
            name: "execute_js",
            description: "Execute JavaScript on the page",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "script": { "type": "string" }
                },
                "required": ["script"],
                "additionalProperties": false
            }),
            required_permission: PermissionMode::DangerFullAccess,
        },
        ToolSpec {
            name: "hover",
            description: "Hover over an element",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "selector": { "type": "string" }
                },
                "required": ["selector"],
                "additionalProperties": false
            }),
            required_permission: PermissionMode::DangerFullAccess,
        },
        ToolSpec {
            name: "press_key",
            description: "Press a keyboard key",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "key": { "type": "string" },
                    "selector": { "type": "string" }
                },
                "required": ["key"],
                "additionalProperties": false
            }),
            required_permission: PermissionMode::DangerFullAccess,
        },
        ToolSpec {
            name: "switch_tab",
            description: "Switch to a different browser tab",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "index": { "type": "integer" }
                },
                "additionalProperties": false
            }),
            required_permission: PermissionMode::DangerFullAccess,
        },
        ToolSpec {
            name: "list_resources",
            description: "List page resources (links, images, forms)",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "type_pattern": { "type": "string" },
                    "name_pattern": { "type": "string" }
                },
                "additionalProperties": false
            }),
            required_permission: PermissionMode::ReadOnly,
        },
        ToolSpec {
            name: "save_file",
            description: "Save a file to the workspace",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "url": { "type": "string" },
                    "filename": { "type": "string" },
                    "subdir": { "type": "string" }
                },
                "required": ["url"],
                "additionalProperties": false
            }),
            required_permission: PermissionMode::WorkspaceWrite,
        },
    ]
}

/// Execute a tool by name with the given JSON input.
///
/// Placeholder — will be populated by subsequent tasks.
///
/// # Errors
///
/// Returns an error string if the tool is not yet implemented.
pub fn execute_tool(tool_name: &str, _input: &serde_json::Value) -> Result<String, String> {
    Err(format!("tool `{tool_name}` not yet implemented"))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::mvp_tool_specs;

    #[test]
    fn mvp_tool_specs_contains_expected_15_tools() {
        let specs = mvp_tool_specs();
        assert_eq!(specs.len(), 15);

        let names: BTreeSet<_> = specs.iter().map(|spec| spec.name).collect();
        assert_eq!(names.len(), 15, "tool names should be unique");
        assert!(names.contains("navigate"));
        assert!(names.contains("save_file"));
    }

    #[test]
    fn every_tool_schema_is_json_object_schema() {
        for spec in mvp_tool_specs() {
            assert_eq!(spec.input_schema["type"], "object", "tool: {}", spec.name);
            assert!(
                spec.input_schema.get("properties").is_some(),
                "tool: {}",
                spec.name
            );
        }
    }
}
