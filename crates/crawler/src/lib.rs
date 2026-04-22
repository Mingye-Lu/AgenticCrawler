use serde_json::json;

pub mod agent;
pub mod browser;
pub mod fetcher;
pub mod manager;
pub mod output;
pub mod playwright;
pub mod prompt;
pub mod shared_client;
pub mod state;
pub mod tool_registry;
pub mod tools;

pub use agent::{AgentHandle, AgentState, CrawlAgent, CrawlError, CrawlResult, CrawlerAgent};
pub use browser::BrowserContext;
pub use fetcher::{FetchError, FetchRouter, FetchedPage};
pub use manager::{AgentInfo, AgentManager, AgentStatus, ForkLimitError, SharedAgentManager};
pub use output::{write_output, OutputError, OutputFormat};
pub use playwright::{PageInfo, PlaywrightBridge, PlaywrightBridgeError, SharedBridge};
pub use shared_client::SharedApiClient;
pub use state::CrawlState;
pub use tool_registry::{ToolHandler, ToolRegistry};

/// Specification for a single tool that the agent can invoke.
pub struct ToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: serde_json::Value,
    /// Extended usage guidance rendered into the system prompt.
    pub instructions: Option<&'static str>,
}

/// Returns the built-in tool specifications.
#[must_use]
#[allow(clippy::too_many_lines)]
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

            instructions: Some("Always use full URLs including the protocol (https://). The response includes extracted page text — read it before taking further actions."),
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

            instructions: Some("May trigger navigation or page changes. Read the tool result before issuing another action."),
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

            instructions: Some("Pass all field values as a JSON object in `fields`. Set `submit` to true to submit the form after filling. Use `form_selector` when the page has multiple forms."),
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

            instructions: Some("Describe what to extract in `instruction`. Pass a JSON template in `data` showing the desired output shape. Return structured JSON."),
        },
        ToolSpec {
            name: "screenshot",
            description: "Capture a screenshot of the current page",
            input_schema: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),

            instructions: Some("Use to verify page state when uncertain about what is visible, or to debug unexpected tool results."),
        },
        ToolSpec {
            name: "go_back",
            description: "Navigate back to the previous page",
            input_schema: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),

            instructions: None,
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

            instructions: Some("Use to reveal lazy-loaded content or elements below the fold. Scroll down before concluding content is missing."),
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

            instructions: Some("Use after actions that trigger page changes (form submits, AJAX requests). Pass `selector` to wait for an element, or `seconds` for a fixed delay."),
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

            instructions: Some("Provide `selector` for the <select> element, then one of `value`, `label`, or `index` to identify the option."),
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

            instructions: Some("Last resort for complex interactions that CSS selectors cannot handle. Prefer click, fill_form, and select_option first. The script runs in the page context and can return a value."),
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

            instructions: None,
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

            instructions: None,
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

            instructions: None,
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

            instructions: Some("Use to discover available links, forms, or images before interacting with them. Helps plan the next action when the page structure is unclear."),
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

            instructions: Some("Downloads the resource at `url` into the workspace directory. Optionally specify `filename` and `subdir`."),
        },
        ToolSpec {
            name: "fork",
            description: "Spawn a parallel subagent on a new browser tab to explore a URL or complete a sub-goal independently. Results are merged when the subagent completes.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "sub_goal": { "type": "string" },
                    "url": { "type": "string" }
                },
                "required": ["sub_goal"],
                "additionalProperties": false
            }),

            instructions: Some("Use fork when you need to visit multiple pages in parallel — e.g., scraping pagination, exploring search results, or comparing products. Each subagent has a step budget and works independently. Fork multiple subagents before waiting for any of them."),
        },
        ToolSpec {
            name: "wait_for_subagents",
            description: "Wait for all active subagents to complete and collect their results. Returns immediately if no subagents are active.",
            input_schema: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),

            instructions: Some("Only call this when you need subagent results before deciding your next action. The done tool automatically waits for subagents, so you do not need to call wait_for_subagents before done."),
        },
        ToolSpec {
            name: "done",
            description: "Signal that the current task is complete. Automatically waits for any active subagents and merges their extracted data.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "summary": { "type": "string" }
                },
                "required": ["summary"],
                "additionalProperties": false
            }),

            instructions: Some("Call when the goal is fully met. Provide a clear summary of what was accomplished and any data extracted. Do not call done until you have completed all necessary work."),
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
    fn mvp_tool_specs_contains_expected_18_tools() {
        let specs = mvp_tool_specs();
        assert_eq!(specs.len(), 18);

        let names: BTreeSet<_> = specs.iter().map(|spec| spec.name).collect();
        assert_eq!(names.len(), 18, "tool names should be unique");
        assert!(names.contains("navigate"));
        assert!(names.contains("save_file"));
        assert!(names.contains("fork"));
        assert!(names.contains("wait_for_subagents"));
        assert!(names.contains("done"));
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
