pub mod action_cache;
pub mod agent;
pub mod child_events;
pub mod confidence;
pub mod failure_classifier;
pub mod loop_detector;
pub mod manager;
pub mod output;
pub mod page_fingerprint;
pub mod prompt;
pub mod registry;
pub mod script_executor;
pub mod script_manager;
pub mod self_healing;
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
pub use prompt::{build_system_prompt, DynamicPromptContext};
pub use registry::{ToolHandler, ToolRegistry};
pub use shared_client::SharedApiClient;
pub use state::{ChildBlock, CrawlState};
pub use url_claim::{ClaimConflict, ClaimGuard, UrlClaimRegistry};

use serde_json::json;

#[cfg(test)]
pub(crate) fn test_async_env_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

fn navigation_tools() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "navigate",
            description: "Navigate to a URL and return the page content as fit_markdown (default, prunes boilerplate for token efficiency), structured markdown, plain text, or raw HTML. Automatically escalates from fast HTTP fetch to full headless browser when JavaScript rendering is detected (React, Next.js, Vue, Angular markers, or short <noscript> bodies). Returns content with an embedded page_map of headings, links, forms, and interactive elements for subsequent tool calls. Use this as the primary tool for accessing any web page.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "url": { "type": "string", "description": "Fully qualified URL to navigate to (must include protocol, e.g. https://example.com). Relative URLs are not supported." },
                    "format": { "type": "string", "enum": ["markdown", "text", "html", "fit_markdown"], "description": "Output format for page content. 'fit_markdown' (default) prunes boilerplate (navs, footers, ads) before conversion for ~40% token savings; 'markdown' preserves full content with structure; 'text' strips all formatting; 'html' returns raw source. Use 'markdown' if fit_markdown seems to be missing important content." },
                    "content_depth": { "type": "string", "enum": ["full", "main", "slim", "none"], "default": "main", "description": "Controls how much page content to return. 'main' (default) extracts article/main content only; 'full' returns everything; 'slim' returns first 2000 chars of main content; 'none' skips content entirely (returns page_map only)." },
                    "strip_images": { "type": "boolean", "default": true, "description": "If true (default), removes image references from output to save context tokens. Set false only when you need image URLs or alt text." },
                    "page_map_depth": { "type": "string", "enum": ["full", "slim", "none"], "default": "slim", "description": "Controls page_map verbosity. 'slim' (default) omits CSS selectors from elements (use @eN refs instead); 'full' includes raw CSS selectors for all elements; 'none' omits the page_map entirely." }
                },
                "required": ["url"],
                "additionalProperties": false
            }),
            instructions: Some("Always use full URLs including the protocol (https://). Returns page content with an embedded page_map showing page structure. Use content_depth to control context size: 'main' (default) extracts article/main content only, 'full' returns everything, 'slim' gives first 2000 chars of main content, 'none' skips content (page_map only). Images are stripped by default (strip_images=true) since they waste context — set false only when you need image URLs. The page_map.links array lets you navigate to linked pages without clicking. The default format is fit_markdown which prunes boilerplate (ads, navs, sidebars) before conversion, saving tokens. Fall back to 'markdown' or 'text' only if content seems missing. page_map_depth controls structural data verbosity: 'slim' (default) strips CSS selectors from all elements to save tokens — use @eN refs for interaction instead. Set 'full' only when you need raw selectors, 'none' to omit page_map entirely."),
        },
        ToolSpec {
            name: "go_back",
            description: "Navigate the browser back to the previous page in history (equivalent to the browser back button). Returns the URL navigated to and a page_state object with headings, landmarks, and links of the resulting page. Use after clicking into a page to return to a listing or search results without re-navigating by URL.",
            input_schema: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
            instructions: Some("Returns the URL navigated to and a `page_state` object with headings, landmarks, and links of the resulting page. Use page_state to understand what you landed on after going back."),
        },
        ToolSpec {
            name: "refresh",
            description: "Reload the current page. Returns page_state after reload. Use after setting intercept rules to replay the page with rules active. Seq increments for temporal observation queries.",
            input_schema: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
            instructions: Some("Reloads the current page and returns a `page_state` object with the updated page structure. Use after setting intercept rules to replay the page with rules active. The seq field increments for temporal observation queries."),
        },
        ToolSpec {
            name: "scroll",
            description: "Scroll the current page up or down by a specified pixel amount to reveal content beyond the visible viewport. Returns updated page_state after scrolling, reflecting any newly loaded lazy content. Use to reveal below-the-fold content, trigger infinite scroll loading, or navigate long pages section by section.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "direction": { "type": "string", "enum": ["up", "down"], "description": "Scroll direction. 'down' reveals content below the viewport; 'up' scrolls back toward the top." },
                    "pixels": { "type": "integer", "description": "Number of pixels to scroll (default: 500). Use 300–800 for a normal page scroll; larger values for quickly reaching page bottom." }
                },
                "additionalProperties": false
            }),
            instructions: Some("Use to reveal lazy-loaded content or elements below the fold. Returns updated page structure after scrolling. Scroll down before concluding content is missing."),
        },
        ToolSpec {
            name: "switch_tab",
            description: "Switch the browser focus to a different open tab by its zero-based index. Returns the tab count and a page_state object reflecting the switched-to tab's content (headings, landmarks, links). Use to access pages opened by link targets, popups, or forked sub-agents without re-navigating.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "index": { "type": "integer", "description": "Zero-based tab index to switch to (0 = first tab). Use the tab count from previous responses to determine valid indices." }
                },
                "additionalProperties": false
            }),
            instructions: Some("Returns tab count and a `page_state` object reflecting the switched-to tab's content (headings, landmarks, links). Use page_state to orient yourself in the new tab without needing a separate page_map call."),
        },
        ToolSpec {
            name: "wait",
            description: "Wait for a DOM element to reach a specified state (visible, hidden, attached, detached) or pause for a fixed duration. Use after actions that trigger asynchronous page changes such as form submissions, AJAX requests, or animations. Returns post-action page_state showing the resulting URL, title, and structural diff once the condition is met or the timeout expires.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "selector": { "type": "string", "description": "CSS selector of the element to wait for (e.g. \".results-loaded\", \"#spinner\"). Mutually exclusive with 'seconds' — provide one or the other." },
                    "seconds": { "type": "number", "description": "Fixed number of seconds to wait (max 300). Use when no specific element signals completion. Mutually exclusive with 'selector'." },
                    "state": {
                        "type": "string",
                        "enum": ["visible", "hidden", "attached", "detached"],
                        "description": "Target state to wait for. 'attached' (default) = element exists in DOM; 'visible' = element is rendered and not hidden; 'hidden' = element is no longer visible; 'detached' = element removed from DOM. Use 'hidden' to wait for loading spinners to disappear."
                    }
                },
                "additionalProperties": false
            }),
            instructions: Some("Use after actions that trigger page changes (form submits, AJAX requests). Pass `selector` to wait for an element, or `seconds` for a fixed delay. Use `state: \"visible\"` to wait until the element is actually visible (not just in the DOM). Use `state: \"hidden\"` to wait for an element to disappear (e.g. a loading spinner). Returns post-action page_state (URL, title, and structural diff) so you can see what changed without a separate page_map call."),
        },
    ]
}

fn interaction_tools() -> Vec<ToolSpec> {
    vec![
        click_tool(),
        click_at_tool(),
        fill_form_tool(),
        select_option_tool(),
        hover_tool(),
        press_key_tool(),
        execute_js_tool(),
    ]
}

fn extraction_tools() -> Vec<ToolSpec> {
    vec![
        page_map_tool(),
        read_content_tool(),
        list_resources_tool(),
        screenshot_tool(),
        save_file_tool(),
        page_performance_tool(),
        inspect_cookies_tool(),
        inspect_storage_tool(),
        audit_accessibility_tool(),
    ]
}

fn click_tool() -> ToolSpec {
    ToolSpec {
        name: "click",
        description: "Click on a page element identified by CSS selector or @eN reference from page_map. May trigger navigation, form submission, or dynamic content changes. Returns post-action page_state showing the resulting URL, title, and structural diff. Prefer navigate with a direct URL from page_map.links when available — use click only for buttons, toggles, and elements without direct links.",
        input_schema: json!({
            "type": "object",
            "properties": {
                "selector": { "type": "string", "description": "CSS selector or @eN element reference from page_map (e.g. \"@e3\", \"button.submit\", \"#login-btn\"). Use @eN refs for stability." }
            },
            "required": ["selector"],
            "additionalProperties": false
        }),
        instructions: Some("May trigger navigation or page changes. The response includes post-action page state (URL, title, page structure) so you can see what changed. Use navigate with a direct URL from page_map.links instead of click when possible — it's more reliable. The selector field accepts CSS selectors or @eN element refs from page_map output (e.g., \"@e3\")."),
    }
}

fn click_at_tool() -> ToolSpec {
    ToolSpec {
        name: "click_at",
        description: "Click at specific viewport coordinates (x, y) using a real mouse event. Use exclusively for elements without stable CSS selectors: canvas drawings, interactive maps, SVG regions, or coordinate-based UIs. Returns post-action page_state. Prefer the selector-based 'click' tool for normal DOM elements — it is more reliable and does not require coordinate calculation.",
        input_schema: json!({
            "type": "object",
            "properties": {
                "x": { "type": "number", "description": "X coordinate in viewport pixels (0 = left edge). Obtain via execute_js with getBoundingClientRect() on the target element." },
                "y": { "type": "number", "description": "Y coordinate in viewport pixels (0 = top edge). Obtain via execute_js with getBoundingClientRect() on the target element." }
            },
            "required": ["x", "y"],
            "additionalProperties": false
        }),
        instructions: Some("Dispatches a real mouse click at the given viewport coordinates using Playwright's page.mouse.click(). Coordinates are relative to the top-left corner of the viewport. To find coordinates, use execute_js with getBoundingClientRect() on the target element — this gives exact pixel positions. Do NOT rely on screenshot to estimate coordinates visually. Prefer the selector-based 'click' tool for normal elements — use click_at only when elements lack stable selectors (canvas drawings, interactive maps, SVG regions, coordinate-based UIs)."),
    }
}

fn fill_form_tool() -> ToolSpec {
    ToolSpec {
        name: "fill_form",
        description: "Fill one or more form fields with values and optionally submit the form. Accepts field identifiers as CSS selectors, field names/IDs, or @eN references from page_map. Returns post-action page_state with the resulting URL and structural diff. Use form_selector to disambiguate when the page contains multiple forms.",
        input_schema: json!({
            "type": "object",
            "properties": {
                "fields": {
                    "type": "object",
                    "additionalProperties": { "type": "string" },
                    "description": "Map of field identifiers to values. Keys can be CSS selectors (\"input[name='email']\"), field name/ID attributes (\"email\"), or @eN refs from page_map (\"@e5\"). Values are the text to type into each field."
                },
                "submit": { "type": "boolean", "description": "If true, submit the form after filling all fields (triggers form submission event). Default: false." },
                "form_selector": { "type": "string", "description": "CSS selector or @eN ref targeting a specific <form> element. Required when the page has multiple forms to disambiguate which form to fill." }
            },
            "required": ["fields"],
            "additionalProperties": false
        }),
        instructions: Some("Keys in `fields` accept CSS selectors, plain field names/IDs, or @eN element refs from page_map (e.g., {\"@e5\": \"value\"}). Set `submit` to true to submit after filling. Use `form_selector` when the page has multiple forms. The response includes post-action page state showing the resulting URL and page structure."),
    }
}

fn select_option_tool() -> ToolSpec {
    ToolSpec {
        name: "select_option",
        description: "Select an option from a <select> dropdown element. Identify the target dropdown via CSS selector or @eN ref, then specify which option to select by its value attribute, visible label text, or zero-based index. Returns post-action page_state showing any page changes triggered by the selection (e.g. dependent dropdowns updating).",
        input_schema: json!({
            "type": "object",
            "properties": {
                "selector": { "type": "string", "description": "CSS selector or @eN ref targeting the <select> element (e.g. \"@e4\", \"select#country\", \"select[name='size']\")." },
                "value": { "type": "string", "description": "The value attribute of the <option> to select (e.g. \"us\", \"medium\"). Use when you know the option's value." },
                "label": { "type": "string", "description": "The visible text of the <option> to select (e.g. \"United States\", \"Medium\"). Use when you know the display text." },
                "index": { "type": "integer", "description": "Zero-based index of the <option> to select (0 = first option). Use when value/label are unknown." }
            },
            "required": ["selector"],
            "additionalProperties": false
        }),
        instructions: Some("Provide `selector` for the <select> element, then one of `value`, `label`, or `index` to identify the option. Returns a `page_state` with the updated page structure after selection. The selector field accepts CSS selectors or @eN element refs from page_map output (e.g., \"@e4\")."),
    }
}

fn hover_tool() -> ToolSpec {
    ToolSpec {
        name: "hover",
        description: "Hover the mouse over a page element to trigger hover-dependent UI such as tooltips, dropdown menus, or expandable content. Returns post-action page_state showing any newly revealed elements. Use this before click when content only appears on mouseover; use click instead if the element needs activation rather than hover.",
        input_schema: json!({
            "type": "object",
            "properties": {
                "selector": { "type": "string", "description": "CSS selector or @eN element reference from page_map targeting the element to hover over (e.g. \"@e2\", \".menu-trigger\", \"nav li\")." }
            },
            "required": ["selector"],
            "additionalProperties": false
        }),
        instructions: Some("Use to reveal tooltips, dropdown menus, or hidden content. Returns a `page_state` with the updated page structure after hover. The selector field accepts CSS selectors or @eN element refs from page_map output (e.g., \"@e2\")."),
    }
}

fn press_key_tool() -> ToolSpec {
    ToolSpec {
        name: "press_key",
        description: "Dispatch a keyboard key press event on the page or a targeted element. Supports named keys (Enter, Escape, Tab, ArrowDown, Backspace) and character keys. Returns post-action page_state reflecting any DOM changes caused by the keypress. Use for form submission (Enter), closing modals (Escape), focus navigation (Tab), or keyboard shortcuts.",
        input_schema: json!({
            "type": "object",
            "properties": {
                "key": { "type": "string", "description": "Key to press — use Playwright key names: \"Enter\", \"Escape\", \"Tab\", \"ArrowDown\", \"ArrowUp\", \"Backspace\", \"Space\", or single characters like \"a\". Modifier combos: \"Control+a\", \"Shift+Tab\"." },
                "selector": { "type": "string", "description": "Optional CSS selector or @eN ref to focus before pressing the key. If omitted, the key is dispatched to the currently focused element or the page." }
            },
            "required": ["key"],
            "additionalProperties": false
        }),
        instructions: Some("Press Enter to submit, Escape to close modals, Tab to move focus, or arrow keys to navigate. Optional `selector` targets a specific element (accepts CSS selectors or @eN refs from page_map). Returns a `page_state` with the updated page structure after the keypress."),
    }
}

fn execute_js_tool() -> ToolSpec {
    ToolSpec {
        name: "execute_js",
        description: "Execute arbitrary JavaScript in the page context and return the evaluation result. The script runs synchronously in the browser's main frame with full access to the DOM, window, and page APIs. Use as a last resort when CSS selectors and other tools cannot achieve the interaction — prefer click, fill_form, and select_option for standard interactions.",
        input_schema: json!({
            "type": "object",
            "properties": {
                "script": { "type": "string", "description": "JavaScript code to execute in the page context. The return value of the last expression is serialized as JSON and returned. For async operations, use 'await' (the script is wrapped in an async function). Example: \"document.title\" or \"await fetch('/api/data').then(r => r.json())\"." }
            },
            "required": ["script"],
            "additionalProperties": false
        }),
        instructions: Some("Last resort for complex interactions that CSS selectors cannot handle. Prefer click, fill_form, and select_option first. The script runs in the page context and can return a value."),
    }
}

fn page_map_tool() -> ToolSpec {
    ToolSpec {
        name: "page_map",
        description: "Get a comprehensive structural map of the current page including headings (h1–h6 with section sizes), landmark regions, forms with field details, links (text + href, capped at 50), and interactive elements (buttons, inputs, selects with state and @eN refs). Use to discover page structure before interacting, or with scope to inspect a specific modal/dialog without background noise. Each interactive element returns a stable @eN reference for use in click, fill_form, hover, press_key, and select_option.",
        input_schema: json!({
            "type": "object",
            "properties": {
                "scope": {
                    "type": "string",
                    "description": "CSS selector to scope all queries within (e.g. \"[role='dialog']\" for modal content only, \".sidebar\" for a specific region). If omitted, queries the full page."
                }
            },
            "additionalProperties": false
        }),
        instructions: Some("Returns the full page anatomy: heading hierarchy (h1-h6 with section sizes), landmark regions, forms (with field details), links (text + href, capped at 50), interactive elements (buttons/inputs/selects with their text, selector, and state like disabled/aria-pressed/aria-expanded), and page metadata. Use scope to inspect only a modal/dialog/overlay without noise from the background page. Use links[].href with navigate instead of clicking when the URL is visible. Each interactive element includes a `ref` field (@e1, @e2, ...) — use these stable handles in click, fill_form, hover, press_key, and select_option instead of copying full CSS selectors."),
    }
}

fn read_content_tool() -> ToolSpec {
    ToolSpec {
        name: "read_content",
        description: "Extract plain text content from a specific page section identified by heading name or CSS selector. Supports pagination via offset and max_chars for large sections. Returns content, total character count, and whether more content is available. Use after page_map to read specific sections without re-fetching the entire page; use navigate instead when you need the full page content.",
        input_schema: json!({
            "type": "object",
            "properties": {
                "heading": {
                    "type": "string",
                    "description": "Exact heading text to find (case-insensitive match). Extracts all content under that heading until the next heading of equal or higher level. If not found, the response lists available headings as a hint."
                },
                "selector": {
                    "type": "string",
                    "description": "CSS selector to extract content from (e.g. \".article-body\", \"#main-content\"). Use when content isn't under a heading or you need a precise DOM target."
                },
                "offset": {
                    "type": "integer",
                    "description": "Character offset to start reading from (default: 0). Use with max_chars to paginate through large sections."
                },
                "max_chars": {
                    "type": "integer",
                    "description": "Maximum characters to return (default: 10000). Reduce for token efficiency; increase to get more content in one call."
                }
            },
            "required": [],
            "additionalProperties": false
        }),
        instructions: Some("Extracts plain text content from a page section. Provide 'heading' for heading-based extraction (exact case-insensitive match) or 'selector' for CSS selector-based extraction. At least one of heading or selector is required. Returns: content, found, total_chars, offset, has_more, truncated, matches_count. If heading not found, found=false and hint lists available headings. Use offset+max_chars to paginate large sections."),
    }
}

fn list_resources_tool() -> ToolSpec {
    ToolSpec {
        name: "list_resources",
        description: "List all discoverable resources on the current page: links (with href and text), images (with src and alt), and forms (with action and method). Returns the complete set without caps — use this when page_map's 50-link limit is insufficient or when you need image URLs. No parameters required.",
        input_schema: json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        }),
        instructions: Some("Use to discover available links, images, or forms. Note: page_map now also includes links and forms — use list_resources when you need ALL links (page_map caps at 50) or image details."),
    }
}

fn screenshot_tool() -> ToolSpec {
    ToolSpec {
        name: "screenshot",
        description: "Capture a screenshot of the current page viewport, a specific element, or the full scrollable page. Returns base64-encoded image data by default, or saves to disk when save=true. Use as a LAST RESORT only after text-based tools (page_map, read_content, execute_js) have failed to provide the needed information — screenshots are expensive and cannot be searched or parsed programmatically.",
        input_schema: json!({
            "type": "object",
            "properties": {
                "selector": {
                    "type": "string",
                    "description": "CSS selector to screenshot a specific element (e.g. \"#chart\", \".product-image\"). If omitted, captures the full viewport."
                },
                "full_page": {
                    "type": "boolean",
                    "description": "If true, capture the full scrollable page height (not just the visible viewport). Ignored when selector is provided. Default: false."
                },
                "format": {
                    "type": "string",
                    "enum": ["png", "jpeg", "webp"],
                    "description": "Image format. 'png' (default) supports transparency; 'jpeg'/'webp' produce smaller files for photos. Default: png."
                },
                "quality": {
                    "type": "integer",
                    "description": "Compression quality 0–100 for jpeg/webp formats only (ignored for png). Lower values = smaller files. Default: 80."
                },
                "save": {
                    "type": "boolean",
                    "description": "If true, save the screenshot to the output directory and return the file path instead of base64 data. Default: false."
                },
                "filename": {
                    "type": "string",
                    "description": "Custom filename for the saved image (e.g. \"homepage.png\"). Only used when save=true. Defaults to a timestamped name if omitted."
                },
                "output_dir": {
                    "type": "string",
                    "description": "Directory to save the screenshot in (absolute or relative to CWD). Only used when save=true. Overrides the default output directory."
                }
            },
            "additionalProperties": false
        }),
        instructions: Some("LAST RESORT. Try page_map, read_content, list_resources, and execute_js FIRST. Use screenshot ONLY after all text-based and JS-based approaches have failed to get the information you need — e.g. verifying purely visual layout, checking how images render, or debugging CSS issues that no other tool can diagnose. NEVER use screenshot to read text, find elements, or identify coordinates. When you do use it: selector targets a specific element, full_page captures below the fold, format=jpeg with quality for smaller files."),
    }
}

fn save_file_tool() -> ToolSpec {
    ToolSpec {
        name: "save_file",
        description: "Download a resource from a URL and save it to the local output directory. Handles any file type (images, PDFs, CSVs, etc.) via HTTP GET. Returns the absolute path of the saved file. Use to persist crawl artifacts; path traversal is blocked for security.",
        input_schema: json!({
            "type": "object",
            "properties": {
                "url": { "type": "string", "description": "Fully qualified URL of the resource to download (e.g. \"https://example.com/report.pdf\"). Must include protocol." },
                "filename": { "type": "string", "description": "Custom filename to save as (e.g. \"report.pdf\"). If omitted, derived from the URL's last path segment. Path traversal characters (../) are rejected." },
                "subdir": { "type": "string", "description": "Subdirectory within the output directory to save into (e.g. \"images\", \"data/csv\"). Created automatically if it doesn't exist." },
                "output_dir": {
                    "type": "string",
                    "description": "Override the default output directory. Can be relative (resolved against CWD) or absolute. If omitted, uses the configured output_dir from settings."
                }
            },
            "required": ["url"],
            "additionalProperties": false
        }),
        instructions: Some("Downloads the resource at `url` into the output directory. Optionally specify `filename`, `subdir`, and `output_dir`."),
    }
}

fn page_performance_tool() -> ToolSpec {
    ToolSpec {
        name: "get_page_performance",
        description: "Get page performance metrics using Navigation Timing and Resource Timing APIs. Returns TTFB, DOM timings, and a breakdown of the top 20 resources by transfer size. Works on both browsers and SPAs.",
        input_schema: json!({
            "type": "object",
            "properties": {},
            "required": [],
            "additionalProperties": false
        }),
        instructions: Some("Captures performance metrics from the current page using the Navigation Timing and Resource Timing APIs. Returns navigation timings (TTFB, DOM interactive/complete, load event), the top 20 resources sorted by transfer size, and a summary with totals and largest/slowest resources. No parameters required."),
    }
}

fn inspect_cookies_tool() -> ToolSpec {
    ToolSpec {
        name: "inspect_cookies",
        description: "Inspect cookies on the current page with security analysis. Returns all cookies with domain, path, expiry, secure/httponly flags, and detected security issues (missing_secure, missing_httponly, sameSite_none_without_secure, excessive_lifetime, overly_broad_domain). Includes third-party detection and filtering options.",
        input_schema: json!({
            "type": "object",
            "properties": {
                "domain": { "type": "string", "description": "Filter by domain substring" },
                "issues_only": { "type": "boolean", "default": false, "description": "If true, return only cookies with detected security issues" }
            },
            "required": [],
            "additionalProperties": false
        }),
        instructions: Some("Returns all cookies with security analysis. Each cookie includes: name, value, domain, path, expires, secure, http_only, same_site, size_bytes, issues (array of detected problems), and third_party flag. Summary includes total count, count with issues, third-party count, session vs persistent breakdown. Use domain filter to narrow results, issues_only to focus on security concerns."),
    }
}

fn inspect_storage_tool() -> ToolSpec {
    ToolSpec {
        name: "inspect_storage",
        description: "Inspect browser storage (localStorage and sessionStorage) on the current page. Returns all key-value pairs with size information. Supports filtering by storage type and key pattern.",
        input_schema: json!({
            "type": "object",
            "properties": {
                "target": { "type": "string", "enum": ["local", "session", "all"], "default": "all", "description": "Which storage to inspect: 'local' for localStorage, 'session' for sessionStorage, 'all' for both" },
                "pattern": { "type": "string", "description": "Filter by key name substring" }
            },
            "required": [],
            "additionalProperties": false
        }),
        instructions: Some("Returns localStorage and/or sessionStorage entries with their values and sizes. Each entry includes: key, value, size_bytes. Summary includes entry counts and total size in KB. Use target to narrow to specific storage type, pattern to filter by key name substring."),
    }
}

fn agent_control_tools() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "fork",
            description: "Spawn a parallel sub-agent on a new browser tab with a declared objective and URL scope. The child agent operates independently with its own step budget and browser context. Sibling forks CANNOT claim overlapping URLs — the call errors atomically with the conflicting owner's ID. Returns the child_id for use with subagent_status, wait_for_subagents, or cancel_subagent. Use to parallelize multi-page crawls (pagination, search results, product comparisons).",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "objective": {
                        "type": "string",
                        "description": "Human-readable goal for the sub-agent (e.g. 'Extract all product titles and prices from this category page'). Should be self-contained — the child has no access to the parent's conversation history."
                    },
                    "scope": {
                        "type": "object",
                        "description": "Declared URL boundary for the child. Prevents sibling overlap. Exactly one of: single_page (one URL), url_list (explicit set), or url_pattern (regex). First-claimer-wins: if a sibling already claimed a URL in this scope, fork fails with the conflicting child_id.",
                        "properties": {
                            "type": { "type": "string", "enum": ["single_page", "url_list", "url_pattern"], "description": "Scope type. 'single_page' for one URL, 'url_list' for a small explicit set, 'url_pattern' for regex-based matching over a subdomain or path prefix." },
                            "url": { "type": "string", "description": "The URL to crawl. Required when type='single_page'." },
                            "urls": {
                                "type": "array",
                                "items": { "type": "string" },
                                "minItems": 1,
                                "description": "List of URLs to crawl (claimed as an all-or-nothing batch). Required when type='url_list'."
                            },
                            "regex": { "type": "string", "description": "Regex pattern matching allowed URLs (e.g. 'https://example\\.com/products/.*'). Must be a valid regex. Required when type='url_pattern'." }
                        },
                        "required": ["type"]
                    },
                    "max_steps": { "type": "integer", "minimum": 1, "description": "Override the child's step budget (default: fork_child_max_steps from settings, typically 15). Increase for complex multi-page tasks." }
                },
                "required": ["objective", "scope"],
                "additionalProperties": false
            }),
            instructions: Some("Use fork to parallelize crawls — e.g., scraping pagination, exploring search results, comparing products. Each subagent gets its own browser tab and step budget. Scope is mandatory: choose single_page for one URL, url_list for a small set, url_pattern (regex) for a navigable subdomain. Siblings CANNOT overlap — if two forks would touch the same URL, the second errors with the conflicting child's id. Pattern overlap is detected only for identical regex strings; subtly different but semantically overlapping patterns (e.g. /posts/.* and /posts/2024/.*) are not caught, so use non-overlapping patterns deliberately. Plan scope upfront to avoid duplicate work. Fork multiple subagents in a row, then poll with subagent_status or wait_for_subagents."),
        },
        ToolSpec {
            name: "wait_for_subagents",
            description: "Block until one or more sub-agents finish and collect their extracted results. Children that complete during the wait have their data merged into the response. Children still running after the timeout (default: 60s) are reported as status='running' and KEEP RUNNING — this tool never cancels or aborts children. Use cancel_subagent explicitly to stop a child you no longer need.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "child_ids": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "List of child IDs to wait for (returned by fork). If omitted, waits for ALL active children. Specify IDs to wait for a subset."
                    }
                },
                "additionalProperties": false
            }),
            instructions: Some("Returns a JSON object: {\"waited\": N, \"finished\": [...], \"still_running\": [...]}. Finished entries include items_extracted and success/error. Still-running entries can be polled again (via another wait_for_subagents) or cancelled (via cancel_subagent). Do NOT assume a timeout means the child failed."),
        },
        ToolSpec {
            name: "subagent_status",
            description: "Non-blocking read-only poll of sub-agent lifecycle state. Returns each child's current step, last tool call, last text output, items extracted count, and seconds since last activity. Never joins, blocks, or cancels — safe to call between any other actions. Use to monitor progress and decide whether to wait, cancel, or fork additional agents.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "child_ids": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "List of child IDs to inspect (returned by fork). If omitted, returns status for ALL tracked children (active and completed)."
                    }
                },
                "additionalProperties": false
            }),
            instructions: Some("Returns {\"children\": [{child_id, sub_goal, state, step, max_steps, last_tool, last_text, items_extracted, last_event_secs_ago, error}, ...]}. State is one of: created, running, completed, failed, cancelled. Use this to decide whether to wait, cancel, or fork more — without consuming the child."),
        },
        ToolSpec {
            name: "cancel_subagent",
            description: "Abort one or more running sub-agents immediately, discarding their in-flight work and partial results. The child's browser tab is closed and its URL claims are released. Use only when you have decided the child's result is no longer needed — there is no way to recover cancelled work. Use wait_for_subagents instead if you want results.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "child_ids": {
                        "type": "array",
                        "items": { "type": "string" },
                        "minItems": 1,
                        "description": "Child IDs to cancel (required, non-empty). Obtain IDs from the fork response or subagent_status."
                    },
                    "reason": {
                        "type": "string",
                        "description": "Optional human-readable reason for cancellation (e.g. 'duplicate work', 'no longer needed'). Logged in the child's Finished event for debugging."
                    }
                },
                "required": ["child_ids"],
                "additionalProperties": false
            }),
            instructions: Some("Cancellation is abortive: the child JoinHandle is aborted and any partial extracted data is discarded. If you want results, call wait_for_subagents instead and let the child finish."),
        },
    ]
}

#[allow(clippy::too_many_lines)]
fn script_management_tools() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "run_script",
            description: "Execute a deterministic multi-step script without per-step LLM round-trips, running on a cloned browser tab. Scripts support loops (for/while/forEach), conditionals (if/else), error handling (try/catch), parallel branches, and variable capture. Returns a script_id immediately — use wait_for_scripts to collect results. Provide either an inline script definition or a name to load a previously saved script. Use when you detect a repetitive pattern (same operation on 3+ pages/items).",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "script": {
                        "type": "object",
                        "description": "Inline script definition object. Must include: version (\"1.0\"), steps (array of node objects: ToolCall, Assign, Collect, Yield, ForLoop, ForEach, WhileLoop, IfElse, TryCatch, Parallel), and optional limits. Use instead of 'name' for new scripts."
                    },
                    "name": {
                        "type": "string",
                        "description": "Name of a previously saved script to run (alternative to inline 'script'). Mutually exclusive with 'script' — provide one or the other."
                    },
                    "save_as": {
                        "type": "string",
                        "description": "Save the script under this name after execution for future reuse (alphanumeric + underscore). Persists to ~/.acrawl/scripts/<name>.json."
                    },
                    "limits": {
                        "type": "object",
                        "description": "Override default execution limits. Keys: max_steps (int), max_timeout_secs (int), max_output_bytes (int), max_parallel_branches (int), per_step_timeout_secs (int)."
                    }
                },
                "additionalProperties": false
            }),
            instructions: Some("Generate and execute a deterministic multi-step script without per-step LLM round-trips. Use when you detect a repetitive pattern (same operation on 3+ pages/items). Workflow: navigate manually first to understand the pattern, then generate a script with for/while loops. Scripts support: tool calls, for/while/if/try-catch/parallel branches, yield checkpoints, and variable capture. Scripts run on a cloned browser tab. Returns a script_id; use wait_for_scripts to collect results. Provide either an inline `script` definition or `name` to load a previously saved script. You can also set `save_as` to persist the executed script and `limits` to override execution limits. Script definition must include: version (\"1.0\"), steps (array of nodes), and limits (max_steps, max_script_size_bytes, max_nesting_depth, max_parallel_branches). Each step is a node: ToolCall (invoke a tool), Assign (set variable), Collect (append to results), Yield (checkpoint), ForLoop/ForEach/WhileLoop (iteration), IfElse (conditional), TryCatch (error handling), or Parallel (concurrent branches)."),
        },
        ToolSpec {
            name: "script_status",
            description: "Check the current execution status of a running or completed script without blocking. Returns the script's state (running, completed, failed, cancelled), current step count, extracted data so far, and any error message. Use to monitor long-running scripts between other actions; use wait_for_scripts to block until completion.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "script_id": {
                        "type": "string",
                        "description": "Script ID returned by run_script (format: scr_XXXXXXXX). Obtain from the run_script response."
                    }
                },
                "required": ["script_id"],
                "additionalProperties": false
            }),
            instructions: Some("Returns the current status of a script: running, completed, failed, or cancelled. Includes step count, extracted data so far, and any error message. Use this to monitor long-running scripts without blocking."),
        },
        ToolSpec {
            name: "wait_for_scripts",
            description: "Block until one or more scripts finish execution and return their collected results. Returns a JSON array of ScriptResult objects with extracted_data, yielded checkpoints, step count, and status. If script_ids is omitted, waits for ALL active scripts. Use after run_script to collect final results.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "script_ids": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "List of script IDs to wait for (format: scr_XXXXXXXX each). If omitted, waits for all currently active scripts to finish."
                    }
                },
                "additionalProperties": false
            }),
            instructions: Some("Blocks until the specified scripts finish (or all active scripts if script_ids is omitted). Returns a JSON array of ScriptResult objects, each containing: script_id, status (completed/failed/cancelled), extracted_data (array of values), yielded_data (checkpoints), step_count, and error (if failed). Use this after run_script to collect results."),
        },
        ToolSpec {
            name: "cancel_script",
            description: "Abort a running script immediately, closing its browser tab and discarding any partial results not yet yielded. The script transitions to 'cancelled' status. Use when a script is stuck, taking too long, or its results are no longer needed.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "script_id": {
                        "type": "string",
                        "description": "Script ID to cancel (format: scr_XXXXXXXX). Obtain from the run_script response or list_scripts."
                    }
                },
                "required": ["script_id"],
                "additionalProperties": false
            }),
            instructions: Some("Cancels a running script. The script's browser page is closed and any partial results are discarded. Returns confirmation of cancellation."),
        },
        ToolSpec {
            name: "save_script",
            description: "Persist a script definition to disk at ~/.acrawl/scripts/<name>.json for reuse across sessions. Once saved, run it later with run_script using name instead of providing the full inline definition. Use for complex extraction patterns you want to apply repeatedly.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Name to save the script under (alphanumeric characters and underscores only, no file extension). Must be unique — overwrites any existing script with the same name."
                    },
                    "script": {
                        "type": "object",
                        "description": "Script definition object (same format as the 'script' parameter in run_script). Must include version, steps, and optionally limits."
                    }
                },
                "required": ["name", "script"],
                "additionalProperties": false
            }),
            instructions: Some("Saves a script definition to ~/.acrawl/scripts/<name>.json for later reuse. Once saved, you can run it again with run_script using name: \"name\" instead of providing the full script object inline. Useful for complex patterns you want to reuse across multiple crawl sessions."),
        },
        ToolSpec {
            name: "list_scripts",
            description: "List all previously saved scripts with their metadata (name, creation date, last modified, size). Returns a JSON array. Use to discover available scripts before running them with run_script by name, or to audit what scripts exist on disk.",
            input_schema: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
            instructions: Some("Returns a JSON array of saved script names and their metadata (creation date, last modified, size). Use this to discover available scripts before running them with run_script + name."),
        },
        ToolSpec {
            name: "read_script",
            description: "Read the full JSON definition of a previously saved script. Returns the complete script object including version, steps, and limits. Use to inspect a saved script's logic before running it, or to understand what an existing script does before modifying and re-saving it.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Name of the saved script to read (as shown in list_scripts output, without .json extension)."
                    }
                },
                "required": ["name"],
                "additionalProperties": false
            }),
            instructions: Some("Returns the full script definition (JSON) for a saved script. Use this to inspect or modify a script before running it, or to understand what a saved script does."),
        },
        ToolSpec {
            name: "set_device",
            description: "Switch browser device emulation between mobile and desktop modes. Recreates the browser context with new viewport, user agent, and touch settings. Cookies and localStorage are preserved. Use preset device names for convenience or provide custom parameters. Returns page_state showing the page as rendered in the new device mode. Cannot be used while sub-agents are running.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "device": {
                        "type": "string",
                        "description": "Device preset name: 'iphone_15', 'iphone_se', 'iphone_15_pro_max', 'pixel_7', 'galaxy_s24', 'ipad_pro', 'ipad', 'galaxy_tab_s9', 'desktop', 'desktop_hd'. Use 'desktop' to reset to default mode. Cannot be combined with custom fields."
                    },
                    "viewport": {
                        "type": "object",
                        "properties": {
                            "width": { "type": "integer", "minimum": 1 },
                            "height": { "type": "integer", "minimum": 1 }
                        },
                        "required": ["width", "height"],
                        "description": "Custom viewport dimensions. Cannot be used with 'device'."
                    },
                    "userAgent": {
                        "type": "string",
                        "description": "Custom user agent string. Cannot be used with 'device'."
                    },
                    "deviceScaleFactor": {
                        "type": "number",
                        "exclusiveMinimum": 0,
                        "description": "Device pixel ratio (e.g., 2 for retina, 3 for iPhone). Cannot be used with 'device'."
                    },
                    "isMobile": {
                        "type": "boolean",
                        "description": "Enable mobile viewport behavior. Cannot be used with 'device'."
                    },
                    "hasTouch": {
                        "type": "boolean",
                        "description": "Enable touch event support. Cannot be used with 'device'."
                    }
                },
                "additionalProperties": false
            }),
            instructions: Some("Provide EITHER a preset name via 'device' (iphone_15, pixel_7, ipad_pro, desktop, etc.) OR one or more custom fields (viewport, userAgent, deviceScaleFactor, isMobile, hasTouch). Do not mix both. Use 'desktop' to reset. Cannot switch device while sub-agents are active."),
        },
    ]
}

/// Returns the built-in tool specifications.
#[must_use]
pub fn mvp_tool_specs() -> Vec<acrawl_core::ToolSpec> {
    let mut specs = navigation_tools();
    specs.extend(interaction_tools());
    specs.extend(extraction_tools());
    specs.extend(agent_control_tools());
    specs.extend(script_management_tools());
    specs
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::mvp_tool_specs;

    #[test]
    fn mvp_tool_specs_contains_expected_30_tools() {
        let specs = mvp_tool_specs();
        assert_eq!(specs.len(), 33);

        let names: BTreeSet<_> = specs.iter().map(|spec| spec.name).collect();
        assert_eq!(names.len(), 33, "tool names should be unique");
        assert!(names.contains("navigate"));
        assert!(names.contains("click_at"));
        assert!(names.contains("save_file"));
        assert!(names.contains("refresh"));
        assert!(names.contains("get_page_performance"));
        assert!(names.contains("inspect_cookies"));
        assert!(names.contains("inspect_storage"));
        assert!(names.contains("fork"));
        assert!(names.contains("wait_for_subagents"));
        assert!(names.contains("cancel_subagent"));
        assert!(names.contains("subagent_status"));
        assert!(names.contains("run_script"));
        assert!(names.contains("script_status"));
        assert!(names.contains("wait_for_scripts"));
        assert!(names.contains("cancel_script"));
        assert!(names.contains("save_script"));
        assert!(names.contains("list_scripts"));
        assert!(names.contains("read_script"));
        assert!(names.contains("set_device"));
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
