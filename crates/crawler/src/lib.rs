use serde_json::json;

mod agent;
mod browser;
pub mod child_events;
mod fetcher;
mod manager;
pub mod markdown;
mod output;
mod playwright;
mod prompt;
mod shared_client;
mod state;
pub mod tool_effect;
mod tool_registry;
mod tools;

pub use agent::{AgentHandle, AgentState, CrawlAgent, CrawlError, CrawlResult, CrawlerAgent};
pub use browser::BrowserContext;
pub use child_events::{ChildControlRegistry, ChildEvent, ChildEventKind, ChildEventSender};
pub use fetcher::{FetchError, FetchRouter, FetchedPage};
pub use manager::{AgentInfo, AgentManager, AgentStatus, ForkLimitError, SharedAgentManager};
pub use output::{write_output, OutputError, OutputFormat};
pub use playwright::{
    BrowserState, PageInfo, PlaywrightBridge, PlaywrightBridgeError, SharedBridge,
};
pub use prompt::build_system_prompt;
pub use shared_client::SharedApiClient;
pub use state::CrawlState;
pub use tool_effect::{ForkSpec, ToolEffect, ToolError, WaitSpec};
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
            description: "Navigate to a URL and get page content as structured markdown (default), plain text, or raw HTML",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "url": { "type": "string" },
                    "format": { "type": "string", "enum": ["markdown", "text", "html"] },
                    "content_depth": { "type": "string", "enum": ["full", "main", "slim", "none"], "default": "main" },
                    "strip_images": { "type": "boolean", "default": true }
                },
                "required": ["url"],
                "additionalProperties": false
            }),
            instructions: Some("Always use full URLs including the protocol (https://). Returns page content with an embedded page_map showing page structure. Use content_depth to control context size: 'main' (default) extracts article/main content only, 'full' returns everything, 'slim' gives first 2000 chars of main content, 'none' skips content (page_map only). Images are stripped by default (strip_images=true) since they waste context — set false only when you need image URLs. The page_map.links array lets you navigate to linked pages without clicking."),
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

            instructions: Some("May trigger navigation or page changes. The response includes post-action page state (URL, title, page structure) so you can see what changed. Use navigate with a direct URL from page_map.links instead of click when possible — it's more reliable."),
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

            instructions: Some("Keys in `fields` can be CSS selectors (`#email`, `input[name=\"q\"]`) or plain field names/IDs that are resolved automatically. Set `submit` to true to submit after filling. Use `form_selector` when the page has multiple forms. The response includes post-action page state showing the resulting URL and page structure."),
        },
        ToolSpec {
            name: "screenshot",
            description: "Capture a screenshot of the current page",
            input_schema: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),

            instructions: Some("Use ONLY when direct text access (page_map, read_content) is insufficient — e.g. verifying visual layout, checking images, or debugging rendering. Never use screenshot to read text content; use read_content instead."),
        },
        ToolSpec {
            name: "go_back",
            description: "Navigate back to the previous page",
            input_schema: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),

            instructions: Some("Returns the URL navigated to and a `page_state` object with headings, landmarks, and links of the resulting page. Use page_state to understand what you landed on after going back."),
        },
        ToolSpec {
            name: "scroll",
            description: "Scroll the page up or down",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "direction": { "type": "string", "enum": ["up", "down"] },
                    "pixels": { "type": "integer", "description": "Pixels to scroll (default: 500). Use 300–800 for a normal page scroll." }
                },
                "additionalProperties": false
            }),

            instructions: Some("Use to reveal lazy-loaded content or elements below the fold. Returns updated page structure after scrolling. Scroll down before concluding content is missing."),
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

            instructions: Some("Provide `selector` for the <select> element, then one of `value`, `label`, or `index` to identify the option. Returns a `page_state` with the updated page structure after selection."),
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

            instructions: Some("Use to reveal tooltips, dropdown menus, or hidden content. Returns a `page_state` with the updated page structure after hover."),
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

            instructions: Some("Press Enter to submit, Escape to close modals, Tab to move focus, or arrow keys to navigate. Optional `selector` targets a specific element. Returns a `page_state` with the updated page structure after the keypress."),
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

            instructions: Some("Returns tab count and a `page_state` object reflecting the switched-to tab's content (headings, landmarks, links). Use page_state to orient yourself in the new tab without needing a separate page_map call."),
        },
        ToolSpec {
            name: "list_resources",
            description: "List page resources (links, images, forms)",
            input_schema: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),

            instructions: Some("Use to discover available links, images, or forms. Note: page_map now also includes links and forms — use list_resources when you need ALL links (page_map caps at 50) or image details."),
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
            name: "page_map",
            description: "Get comprehensive page structure: headings, landmarks, forms, interactive elements, and links",
            input_schema: json!({
                "type": "object",
                "properties": {},
                "required": [],
                "additionalProperties": false
            }),
            instructions: Some("Returns the full page anatomy: heading hierarchy (h1-h6 with section sizes), landmark regions (nav/main/aside/article/footer), forms (with field details), links (text + href, capped at 50), interactive element counts, and page metadata. Use links[].href with navigate instead of clicking when the URL is visible. If headings is empty, the page has no semantic headings — check landmarks instead."),
        },
        ToolSpec {
            name: "read_content",
            description: "Extract text content from a specific page section by heading name or CSS selector",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "heading": {
                        "type": "string",
                        "description": "Exact heading text to find (case-insensitive)"
                    },
                    "selector": {
                        "type": "string",
                        "description": "CSS selector to extract content from"
                    },
                    "offset": {
                        "type": "integer",
                        "description": "Character offset to start reading from (default: 0)"
                    },
                    "max_chars": {
                        "type": "integer",
                        "description": "Maximum characters to return (default: 10000)"
                    }
                },
                "required": [],
                "additionalProperties": false
            }),
            instructions: Some("Extracts plain text content from a page section. Provide 'heading' for heading-based extraction (exact case-insensitive match) or 'selector' for CSS selector-based extraction. At least one of heading or selector is required. Returns: content, found, total_chars, offset, has_more, truncated, matches_count. If heading not found, found=false and hint lists available headings. Use offset+max_chars to paginate large sections."),
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

            instructions: Some("Only call this when you need subagent results before deciding your next action."),
        },
        ToolSpec {
            name: "wait_for_human",
            description: "Pause execution and request human intervention. Use when encountering captchas, login walls, paywalls, or other obstacles that require human action in the browser.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "reason": {
                        "type": "string",
                        "description": "Why human intervention is needed (e.g., 'Login wall detected', 'CAPTCHA challenge appeared')"
                    }
                },
                "required": ["reason"],
                "additionalProperties": false
            }),
            instructions: Some("Use only when you encounter an obstacle that genuinely requires a human to solve (captcha, login, paywall). Be specific in the reason about what the human needs to do. The browser becomes visible for the human. After they finish and press resume, you receive the updated page content (URL, title, text) — continue your task from there."),
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
    fn mvp_tool_specs_contains_expected_19_tools() {
        let specs = mvp_tool_specs();
        assert_eq!(specs.len(), 19);

        let names: BTreeSet<_> = specs.iter().map(|spec| spec.name).collect();
        assert_eq!(names.len(), 19, "tool names should be unique");
        assert!(names.contains("navigate"));
        assert!(names.contains("save_file"));
        assert!(names.contains("fork"));
        assert!(names.contains("wait_for_subagents"));
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

    #[test]
    fn navigate_spec_has_format_param() {
        let specs = mvp_tool_specs();
        let nav = specs.iter().find(|s| s.name == "navigate").unwrap();
        let schema_str = nav.input_schema.to_string();
        assert!(
            schema_str.contains("format"),
            "navigate schema should have format param"
        );
        assert!(
            schema_str.contains("markdown"),
            "format enum should include markdown"
        );
    }

    #[test]
    fn page_map_spec_mentions_landmarks() {
        let specs = mvp_tool_specs();
        let pm = specs.iter().find(|s| s.name == "page_map").unwrap();
        assert!(
            pm.description.contains("landmarks"),
            "page_map description should mention landmarks"
        );
        assert!(
            pm.instructions.unwrap_or("").contains("landmark"),
            "page_map instructions should mention landmark"
        );
    }

    #[tokio::test]
    async fn test_cloakbrowser_launch_smoke() {
        use super::{PlaywrightBridge, PlaywrightBridgeError};

        // Self-skip if not in CI and CloakBrowser not installed locally
        let in_ci = std::env::var("CI").is_ok();
        let cloakbrowser_installed = runtime::config_home_dir()
            .join("node_modules")
            .join("cloakbrowser")
            .exists();
        if !in_ci && !cloakbrowser_installed {
            eprintln!("test_cloakbrowser_launch_smoke: skipping — CloakBrowser not installed");
            return;
        }

        // If installed, launch the bridge and verify bootstrap succeeds
        let result = PlaywrightBridge::new().await;
        match result {
            Ok(bridge) => {
                let _ = bridge.close().await;
            }
            Err(PlaywrightBridgeError::PlaywrightNotInstalled(_)) => {
                eprintln!(
                    "test_cloakbrowser_launch_smoke: skipping — CloakBrowser binary not available"
                );
            }
            Err(e) => {
                panic!("Unexpected error launching CloakBrowser bridge: {e}");
            }
        }
    }
}
