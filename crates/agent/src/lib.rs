pub mod agent;
pub mod child_events;
pub mod manager;
pub mod output;
pub mod prompt;
pub mod registry;
mod shared_client;
pub mod state;
pub mod tools;
mod url_claim;

pub mod tool_effect {
    pub use acrawl_core::effect::{
        CancelSpec, CrawlScope, CrawlTask, StatusSpec, ToolEffect, WaitSpec,
    };
    pub use acrawl_core::error::ToolExecutionError;
}

// Re-exports that tools and registry use via `crate::` paths
pub use acrawl_core::effect::{
    CancelSpec, CrawlScope, CrawlTask, StatusSpec, ToolEffect, WaitSpec,
};
pub use acrawl_core::error::ToolExecutionError;
pub use acrawl_core::ToolSpec;
pub use browser::{
    generate_bridge_token, markdown, prune, ws_server, BridgeCommand, BridgeError, BridgeResponse,
    BrowserBackend, BrowserContext, BrowserState, ExtensionBridge, FetchError, FetchRouter,
    FetchedPage, PageInfo, PlaywrightBridge, ScreenshotOptions, SharedBridge, WsBridgeError,
    WsBridgeServer,
};

pub use agent::{AgentHandle, AgentState, CrawlAgent, CrawlError, CrawlResult, CrawlerAgent};
pub use child_events::{
    ChildControlRegistry, ChildEvent, ChildEventKind, ChildEventSender, ChildLifecycle,
    ChildSnapshot, ChildSnapshotRegistry,
};
pub use manager::{AgentInfo, AgentManager, AgentStatus, ForkLimitError, SharedAgentManager};
pub use output::{write_output, OutputError, OutputFormat};
pub use prompt::build_system_prompt;
pub use registry::{ToolHandler, ToolRegistry};
pub use shared_client::SharedApiClient;
pub use state::{ChildBlock, CrawlState};
pub use url_claim::{ClaimConflict, ClaimGuard, UrlClaimRegistry};

/// Returns the built-in tool specifications.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn mvp_tool_specs() -> Vec<acrawl_core::ToolSpec> {
    use acrawl_core::ToolSpec;
    use serde_json::json;
    vec![
        ToolSpec {
            name: "navigate",
            description: "Navigate to a URL and get page content as structured markdown (default), plain text, or raw HTML",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "url": { "type": "string" },
                    "format": { "type": "string", "enum": ["markdown", "text", "html", "fit_markdown"] },
                    "content_depth": { "type": "string", "enum": ["full", "main", "slim", "none"], "default": "main" },
                    "strip_images": { "type": "boolean", "default": true }
                },
                "required": ["url"],
                "additionalProperties": false
            }),
            instructions: Some("Always use full URLs including the protocol (https://). Returns page content with an embedded page_map showing page structure. Use content_depth to control context size: 'main' (default) extracts article/main content only, 'full' returns everything, 'slim' gives first 2000 chars of main content, 'none' skips content (page_map only). Images are stripped by default (strip_images=true) since they waste context \u{2014} set false only when you need image URLs. The page_map.links array lets you navigate to linked pages without clicking. Use format='fit_markdown' to filter low-value DOM nodes before conversion, reducing token consumption on noisy pages."),
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
            instructions: Some("May trigger navigation or page changes. The response includes post-action page state (URL, title, page structure) so you can see what changed. Use navigate with a direct URL from page_map.links instead of click when possible \u{2014} it's more reliable. The selector field accepts CSS selectors or @eN element refs from page_map output (e.g., \"@e3\")."),
        },
        ToolSpec {
            name: "click_at",
            description: "Click at specific viewport coordinates (x, y). Use for canvas, maps, SVGs, or elements without stable CSS selectors",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "x": { "type": "number", "description": "X coordinate in viewport pixels" },
                    "y": { "type": "number", "description": "Y coordinate in viewport pixels" }
                },
                "required": ["x", "y"],
                "additionalProperties": false
            }),
            instructions: Some("Dispatches a real mouse click at the given viewport coordinates using Playwright's page.mouse.click(). Coordinates are relative to the top-left corner of the viewport. To find coordinates, use execute_js with getBoundingClientRect() on the target element \u{2014} this gives exact pixel positions. Do NOT rely on screenshot to estimate coordinates visually. Prefer the selector-based 'click' tool for normal elements \u{2014} use click_at only when elements lack stable selectors (canvas drawings, interactive maps, SVG regions, coordinate-based UIs)."),
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
            instructions: Some("Keys in `fields` accept CSS selectors, plain field names/IDs, or @eN element refs from page_map (e.g., {\"@e5\": \"value\"}). Set `submit` to true to submit after filling. Use `form_selector` when the page has multiple forms. The response includes post-action page state showing the resulting URL and page structure."),
        },
        ToolSpec {
            name: "screenshot",
            description: "Capture a screenshot of the current page or a specific element",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "selector": {
                        "type": "string",
                        "description": "CSS selector to screenshot a specific element. If omitted, captures the full viewport."
                    },
                    "full_page": {
                        "type": "boolean",
                        "description": "Capture the full scrollable page, not just the visible viewport. Ignored when selector is provided."
                    },
                    "format": {
                        "type": "string",
                        "enum": ["png", "jpeg", "webp"],
                        "description": "Image format. JPEG/WebP produce smaller files but no transparency. Default: png."
                    },
                    "quality": {
                        "type": "integer",
                        "description": "Image quality 0-100. Only applies when format is jpeg or webp. Default: 80."
                    },
                    "save": {
                        "type": "boolean",
                        "description": "If true, save the screenshot to the output directory and return the saved path instead of base64."
                    },
                    "filename": {
                        "type": "string",
                        "description": "Filename for the saved image (e.g. \"shot.png\"). Only used when save is true. Defaults to a timestamped name if omitted."
                    },
                    "output_dir": {
                        "type": "string",
                        "description": "Directory to save the screenshot in. Overrides the default output directory."
                    }
                },
                "additionalProperties": false
            }),
            instructions: Some("LAST RESORT. Try page_map, read_content, list_resources, and execute_js FIRST. Use screenshot ONLY after all text-based and JS-based approaches have failed to get the information you need \u{2014} e.g. verifying purely visual layout, checking how images render, or debugging CSS issues that no other tool can diagnose. NEVER use screenshot to read text, find elements, or identify coordinates. When you do use it: selector targets a specific element, full_page captures below the fold, format=jpeg with quality for smaller files."),
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
                    "pixels": { "type": "integer", "description": "Pixels to scroll (default: 500). Use 300\u{2013}800 for a normal page scroll." }
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
                    "seconds": { "type": "number" },
                    "state": {
                        "type": "string",
                        "enum": ["visible", "hidden", "attached", "detached"],
                        "description": "Wait for element to reach this state. Default: attached (exists in DOM)."
                    }
                },
                "additionalProperties": false
            }),
            instructions: Some("Use after actions that trigger page changes (form submits, AJAX requests). Pass `selector` to wait for an element, or `seconds` for a fixed delay. Use `state: \"visible\"` to wait until the element is actually visible (not just in the DOM). Use `state: \"hidden\"` to wait for an element to disappear (e.g. a loading spinner)."),
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
            instructions: Some("Provide `selector` for the <select> element, then one of `value`, `label`, or `index` to identify the option. Returns a `page_state` with the updated page structure after selection. The selector field accepts CSS selectors or @eN element refs from page_map output (e.g., \"@e4\")."),
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
            instructions: Some("Use to reveal tooltips, dropdown menus, or hidden content. Returns a `page_state` with the updated page structure after hover. The selector field accepts CSS selectors or @eN element refs from page_map output (e.g., \"@e2\")."),
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
            instructions: Some("Press Enter to submit, Escape to close modals, Tab to move focus, or arrow keys to navigate. Optional `selector` targets a specific element (accepts CSS selectors or @eN refs from page_map). Returns a `page_state` with the updated page structure after the keypress."),
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
            instructions: Some("Use to discover available links, images, or forms. Note: page_map now also includes links and forms \u{2014} use list_resources when you need ALL links (page_map caps at 50) or image details."),
        },
        ToolSpec {
            name: "save_file",
            description: "Save a file to the output directory",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "url": { "type": "string" },
                    "filename": { "type": "string" },
                    "subdir": { "type": "string" },
                    "output_dir": {
                        "type": "string",
                        "description": "Directory to save the file in. Overrides the default output directory. Can be relative (resolved against CWD) or absolute."
                    }
                },
                "required": ["url"],
                "additionalProperties": false
            }),
            instructions: Some("Downloads the resource at `url` into the output directory. Optionally specify `filename`, `subdir`, and `output_dir`."),
        },
        ToolSpec {
            name: "page_map",
            description: "Get comprehensive page structure: headings, landmarks, forms, interactive elements, and links",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "scope": {
                        "type": "string",
                        "description": "CSS selector to scope all queries within (e.g. \"[role='dialog']\" for modal content only). If omitted, queries the full page."
                    }
                },
                "additionalProperties": false
            }),
            instructions: Some("Returns the full page anatomy: heading hierarchy (h1-h6 with section sizes), landmark regions, forms (with field details), links (text + href, capped at 50), interactive elements (buttons/inputs/selects with their text, selector, and state like disabled/aria-pressed/aria-expanded), and page metadata. Use scope to inspect only a modal/dialog/overlay without noise from the background page. Use links[].href with navigate instead of clicking when the URL is visible. Each interactive element includes a `ref` field (@e1, @e2, ...) \u{2014} use these stable handles in click, fill_form, hover, press_key, and select_option instead of copying full CSS selectors."),
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
            description: "Spawn a parallel subagent on a new browser tab with a typed work packet. The packet declares an `objective` (what to do) and a `scope` (which URLs the child is allowed to claim). Sibling forks CANNOT claim overlapping URLs/patterns \u{2014} the call errors atomically with the conflicting owner.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "objective": {
                        "type": "string",
                        "description": "Human-readable goal for the subagent (e.g., 'extract all product titles')."
                    },
                    "scope": {
                        "type": "object",
                        "description": "Declared work boundary. Exactly one of single_page, url_list, or url_pattern.",
                        "properties": {
                            "type": { "type": "string", "enum": ["single_page", "url_list", "url_pattern"] },
                            "url": { "type": "string", "description": "Required when type=single_page." },
                            "urls": {
                                "type": "array",
                                "items": { "type": "string" },
                                "minItems": 1,
                                "description": "Required when type=url_list."
                            },
                            "regex": { "type": "string", "description": "Required when type=url_pattern. Must compile." }
                        },
                        "required": ["type"]
                    },
                    "max_steps": { "type": "integer", "minimum": 1, "description": "Override the child's step budget." }
                },
                "required": ["objective", "scope"],
                "additionalProperties": false
            }),
            instructions: Some("Use fork to parallelize crawls \u{2014} e.g., scraping pagination, exploring search results, comparing products. Each subagent gets its own browser tab and step budget. Scope is mandatory: choose single_page for one URL, url_list for a small set, url_pattern (regex) for a navigable subdomain. Siblings CANNOT overlap \u{2014} if two forks would touch the same URL, the second errors with the conflicting child's id. Pattern overlap is detected only for identical regex strings; subtly different but semantically overlapping patterns (e.g. /posts/.* and /posts/2024/.*) are not caught, so use non-overlapping patterns deliberately. Plan scope upfront to avoid duplicate work. Fork multiple subagents in a row, then poll with subagent_status or wait_for_subagents."),
        },
        ToolSpec {
            name: "wait_for_subagents",
            description: "Wait for active subagents and return a JSON snapshot of each one's status. Children that finish during the wait have their items collected and merged. Children that have not finished by the timeout are reported as `status: \"running\"` and KEEP RUNNING \u{2014} wait NEVER cancels or aborts. Use `cancel_subagent` if you actually want to stop a child.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "child_ids": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Optional list of child IDs to wait for. Defaults to all active children."
                    }
                },
                "additionalProperties": false
            }),
            instructions: Some("Returns a JSON object: {\"waited\": N, \"finished\": [...], \"still_running\": [...]}. Finished entries include items_extracted and success/error. Still-running entries can be polled again (via another wait_for_subagents) or cancelled (via cancel_subagent). Do NOT assume a timeout means the child failed."),
        },
        ToolSpec {
            name: "subagent_status",
            description: "Read-only poll: returns a JSON snapshot of each subagent's lifecycle, current step, last tool call, last text output, items extracted, and how long ago its last event was observed. Never joins or cancels \u{2014} safe to call between any other actions.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "child_ids": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Optional list of child IDs to inspect. Defaults to all tracked children."
                    }
                },
                "additionalProperties": false
            }),
            instructions: Some("Returns {\"children\": [{child_id, sub_goal, state, step, max_steps, last_tool, last_text, items_extracted, last_event_secs_ago, error}, ...]}. State is one of: created, running, completed, failed, cancelled. Use this to decide whether to wait, cancel, or fork more \u{2014} without consuming the child."),
        },
        ToolSpec {
            name: "cancel_subagent",
            description: "Abort one or more running subagents immediately. Their in-flight work is discarded. Use this only when you have decided the child's result is no longer needed.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "child_ids": {
                        "type": "array",
                        "items": { "type": "string" },
                        "minItems": 1,
                        "description": "Child IDs to cancel (required, non-empty)."
                    },
                    "reason": {
                        "type": "string",
                        "description": "Optional human-readable reason for cancellation (logged in the Finished event)."
                    }
                },
                "required": ["child_ids"],
                "additionalProperties": false
            }),
            instructions: Some("Cancellation is abortive: the child JoinHandle is aborted and any partial extracted data is discarded. If you want results, call wait_for_subagents instead and let the child finish."),
        },
    ]
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::mvp_tool_specs;

    #[test]
    fn mvp_tool_specs_contains_expected_21_tools() {
        let specs = mvp_tool_specs();
        assert_eq!(specs.len(), 21);

        let names: BTreeSet<_> = specs.iter().map(|spec| spec.name).collect();
        assert_eq!(names.len(), 21, "tool names should be unique");
        assert!(names.contains("navigate"));
        assert!(names.contains("click_at"));
        assert!(names.contains("save_file"));
        assert!(names.contains("fork"));
        assert!(names.contains("wait_for_subagents"));
        assert!(names.contains("cancel_subagent"));
        assert!(names.contains("subagent_status"));
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
