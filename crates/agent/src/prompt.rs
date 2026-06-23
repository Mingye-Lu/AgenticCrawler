use acrawl_core::ToolSpec;

#[derive(Debug, Default, Clone)]
pub struct DynamicPromptContext {
    pub stagnation_alert: Option<String>,
    pub planning_guidance: Option<String>,
    pub budget_warning: Option<String>,
    pub loop_nudge: Option<String>,
}

fn format_tool(spec: &ToolSpec) -> String {
    let required: Vec<&str> = spec
        .input_schema
        .get("required")
        .and_then(|r| r.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>())
        .unwrap_or_default();

    let mut line = if required.is_empty() {
        format!("- **{}**: {}", spec.name, spec.description)
    } else {
        format!(
            "- **{}**: {} (params: {})",
            spec.name,
            spec.description,
            required.join(", ")
        )
    };
    if let Some(instructions) = spec.instructions {
        use std::fmt::Write;
        let _ = write!(line, "\n  {instructions}");
    }
    line
}

fn list_tools(specs: &[ToolSpec]) -> String {
    specs.iter().map(format_tool).collect::<Vec<_>>().join("\n")
}

/// Build the crawler system prompt from tool specifications.
///
/// Returns `Vec<String>` for [`runtime::ConversationRuntime`]'s `system_prompt` parameter.
#[must_use]
pub fn build_system_prompt(
    tool_specs: &[ToolSpec],
    dynamic_context: Option<&DynamicPromptContext>,
) -> Vec<String> {
    let mut sections = vec![
        section_identity(tool_specs),
        section_operating_procedure(),
        section_data_integrity(),
        section_constraints(),
        section_error_recovery(),
        section_completion(),
        section_parallel_exploration(),
        section_autonomous_scripts(),
        section_observation_tools(),
    ];

    if let Some(dynamic_section) = dynamic_context.and_then(build_dynamic_section) {
        sections.push(dynamic_section);
    }

    sections
}

#[must_use]
pub fn build_dynamic_section(ctx: &DynamicPromptContext) -> Option<String> {
    let mut items = Vec::new();

    if let Some(value) = &ctx.stagnation_alert {
        items.push(format!("- Stagnation alert: {value}"));
    }
    if let Some(value) = &ctx.planning_guidance {
        items.push(format!("- Planning guidance: {value}"));
    }
    if let Some(value) = &ctx.budget_warning {
        items.push(format!("- Budget warning: {value}"));
    }
    if let Some(value) = &ctx.loop_nudge {
        items.push(format!("- Loop nudge: {value}"));
    }

    if items.is_empty() {
        None
    } else {
        Some(format!("Dynamic guidance:\n{}", items.join("\n")))
    }
}

fn section_identity(tool_specs: &[ToolSpec]) -> String {
    let tool_listing = list_tools(tool_specs);
    format!(
        "You are an autonomous web crawler agent. Your job is to complete the \
         user's web task by navigating websites, interacting with pages, and \
         extracting information from page content only.\n\n\
         Your task arrives as a user message. Read it carefully before acting.\n\n\
         Available browser tools:\n{tool_listing}"
    )
}

fn section_operating_procedure() -> String {
    let blocker_instruction =
        "\x20\x20 b. Check for blockers: if the page content mentions CAPTCHAs, \
     \"verify you are human\", \"unusual traffic\", login forms you cannot fill, \
     paywalls, or access denied messages — stop and report the blocker. \
     Do not retry or work around it.\n";
    format!(
        "Operating procedure:\n\
     1. Read the current task carefully. Identify every concrete requirement.\n\
     2. Start from the most relevant direct URL when known, using a full URL \
     including https://.\n\
     3. At each step:\n\
     \x20\x20 a. Observe the current page state from the last tool result.\n\
     {blocker_instruction}\
     \x20\x20 c. Decide the single best next action.\n\
     \x20\x20 d. Execute one tool call.\n\
     \x20\x20 e. Evaluate the result before continuing.\n\
     4. Prefer the simplest reliable action:\n\
      \x20\x20 - Direct navigate over clicking links when the URL is known.\n\
      \x20\x20 - click, fill_form, and scroll before execute_js.\n\
       \x20\x20 - page_map + read_content + execute_js over screenshot. Screenshot is the \
       LAST RESORT tool \u{2014} use it only after page_map, read_content, list_resources, \
       AND execute_js have all failed to provide the information you need. Valid uses: \
       verifying purely visual layout or debugging CSS rendering. Invalid uses: reading \
       text, finding elements, identifying click coordinates.\n\
       \x20\x20 - Use `set_device` to switch between mobile and desktop browser \
       emulation when a site behaves differently on mobile or when the goal \
       requires mobile-specific content. Use 'desktop' to reset back to default.\n\
      5. When extracting information from a page:\n\
      \x20\x20 a. Call page_map to see the heading structure and section sizes.\n\
      \x20\x20 b. Call read_content with the heading name or CSS selector for the section you need.\n\
      \x20\x20 c. For large sections, use offset and max_chars to paginate through content.\n\
      \x20\x20 d. If page_map returns no sections (empty list), use read_content with a CSS \
      selector (try \"main\", \"article\", \"body\", or \"[role=main]\" as general-purpose selectors).\n\
      \x20\x20 e. If read_content also returns nothing useful, try execute_js to query the DOM directly.\n\
      \x20\x20 f. Do NOT scroll+screenshot to read page content. The text is accessible \
      through read_content regardless of scroll position.\n\
       6. When you have accomplished the goal, provide a summary and the structured results \
       in JSON format in your final message."
    )
}

fn section_data_integrity() -> String {
    "Data integrity:\n\
     - Only report facts that were actually observed through tool results or \
     page content. Never use training knowledge to fill gaps in extracted data.\n\
     - Every URL, price, name, date, and value you report must appear verbatim \
     in a tool result from this session. If information was not found on the \
     page, say so explicitly.\n\
     - Never claim to have clicked, extracted, or observed something unless a \
     tool result confirms it.\n\
     - Distinguish clearly between what you observed on the page and any \
     inferences or assumptions you are making."
        .to_string()
}

fn section_constraints() -> String {
    "Constraints:\n\
     - Do not loop indefinitely.\n\
     - Maximum retries for any single failed action: 2. After that, try a \
     different approach entirely.\n\
     - If you are on the same page for 3 or more steps without meaningful \
     progress, change strategy.\n\
     - If no meaningful progress is made in 3 consecutive steps, stop and \
     summarize partial results.\n\
     - Keep extracted data clean, deduplicated, and well-structured.\n\
     - Prefer navigate with a direct URL over clicking links when possible.\n\
     - Use go_back to return to previous pages instead of re-navigating."
        .to_string()
}

fn section_error_recovery() -> String {
    "Error recovery by situation:\n\
     - Selector not found: Try a more general selector, scroll to reveal \
     content, or check for overlays or popups blocking the element.\n\
     - Page not loading or navigation error: Verify the URL is correct and \
     complete. Try an alternative URL or search for the page.\n\
     - CAPTCHA or human verification (e.g. \"verify you are human\", \
     \"unusual traffic\", reCAPTCHA, hCaptcha, checkbox challenges): \
     Stop and report the blocker in your results. Do NOT retry or try to solve it.\n\
     - reCAPTCHA v3 (invisible, score-based): A navigate result with \
     recaptcha_detected: true is NOT a blocker by itself — keep reading and \
     extracting normally, exactly as you would on any other page. Only when a \
     form submission produces no visible page change (same URL, changed: \
     false) is this the likely cause: the browser is probably headless, \
     reCAPTCHA v3 scores headless sessions low, and the server may silently \
     reject the submission. Do NOT change settings or call any tool to fix \
     this yourself — report it to the user: a human can retry with `acrawl \
     config set headless false` (or --headed), or use the extension bridge \
     (/extension).\n\
     - Login wall or paywall: Stop and report the blocker in your results.\n\
     - Anti-bot detection (403/429 errors, empty page, \"access denied\"): \
     Wait briefly (use the `wait` tool) and retry once. \
     If the obstacle persists after retrying, stop and report the blocker.\n\
     - Empty results on a page expected to have data: Scroll down for \
      lazy-loaded content, wait for dynamic rendering, or check whether the \
      page uses iframes.\n\
      - Empty results on page_map (sections list is empty): the page does not use \
      semantic headings — fall back to read_content with a CSS selector.\n\
      - JavaScript interaction failing: Use execute_js as a fallback when \
      click or fill_form fail, but not as a first choice.\n\
     - Popup or overlay blocking interaction: Try pressing Escape or clicking \
     a dismiss button before retrying the intended action."
        .to_string()
}

fn section_completion() -> String {
    let impossible_instruction =
        "- If the task is impossible to continue (requires login, payment, or \
      access you do not have), stop and explain the blocker.\n";
    format!(
        "Completion:\n\
      - Before reporting success, re-read the original task and confirm each \
      requirement is met by data you actually extracted.\n\
      - If any requirement is unmet or uncertain, say so explicitly rather \
      than guessing.\n\
      {impossible_instruction}\
      - When providing extracted data, include the source URL and note any \
      gaps or limitations."
    )
}

fn section_parallel_exploration() -> String {
    "Parallel exploration:\n\
       - Use the `fork` tool to spawn a subagent on a separate browser tab when you need \
       to explore multiple pages simultaneously.\n\
       - Each subagent gets a copy of your history and works independently.\n\
       - You can fork multiple subagents at once (up to the configured limit).\n\
       - After forking, continue your own work — subagents run in parallel.\n\
       - Use `wait_for_subagents` to block and collect results when you need them.\n\
       - Prefer forking over sequential navigation when visiting multiple independent pages.\n\
       - Example: Scraping 5 product pages? Fork 5 subagents, each visiting one page, then wait for their results."
         .to_string()
}

fn section_autonomous_scripts() -> String {
    "Autonomous scripts:\n\
      When you detect a **repetitive page pattern** (same structure across 3+ URLs/items), switch from per-step LLM navigation to a **deterministic script**. Scripts execute browser tools in loops without LLM round-trips — dramatically faster and cheaper for batch operations.\n\n\
      When to use scripts:\n\
      - After manually navigating 2–3 similar pages and identifying a consistent extraction pattern\n\
      - For pagination scraping (50 product pages with identical structure)\n\
      - For repeated actions across a list (filling N forms, clicking N buttons)\n\n\
      Workflow:\n\
      1. Navigate manually to 2–3 sample pages to understand the pattern\n\
      2. Write the script inline in `run_script` using the patterns you observed\n\
      3. The script returns a `script_id` immediately (non-blocking)\n\
      4. Poll `script_status` to monitor progress or use `wait_for_scripts` to block until done\n\
      5. Collect results from the final `ScriptResult`\n\n\
      Script tools:\n\
      - **`run_script`** — Execute a script inline or by saved name. Returns `script_id`.\n\
      - **`script_status`** — Check real-time status, step count, and yielded data.\n\
      - **`wait_for_scripts`** — Block until script(s) complete. Returns all results.\n\
      - **`cancel_script`** — Abort a running script.\n\
      - **`save_script`** — Save a script definition for reuse.\n\
      - **`list_scripts`** — Show all saved scripts.\n\
      - **`read_script`** — Read a saved script definition.\n\n\
      Example: Scrape 50 product pages\n\
      ```json\n\
      {\n\
        \"schema_version\": 1,\n\
        \"steps\": [\n\
          {\"type\": \"assign\", \"variable\": \"urls\", \"value\": {\"kind\": \"literal\", \"value\": [\"https://shop.example.com/p/1\", \"...\"]}},\n\
          {\"type\": \"for_each\", \"variable\": \"url\", \"iterable\": {\"kind\": \"variable\", \"value\": \"urls\"}, \"steps\": [\n\
            {\"type\": \"tool_call\", \"tool\": \"navigate\", \"input\": {\"url\": \"$url\"}, \"output\": \"page\"},\n\
            {\"type\": \"tool_call\", \"tool\": \"read_content\", \"input\": {\"selector\": \".product-title\"}, \"output\": \"title\"},\n\
            {\"type\": \"collect\", \"value\": {\"kind\": \"variable\", \"value\": \"title\"}},\n\
            {\"type\": \"yield\", \"value\": {\"kind\": \"variable\", \"value\": \"title\"}}\n\
          ]}\n\
        ]\n\
      }\n\
      ```\n\n\
      Scripts support: `for`/`foreach`/`while` loops, `if`/`else` branches, `try`/`catch`/`finally`, `parallel` branches, `yield` checkpoints, `assign` variables, and inline JS via `execute_js`."
        .to_string()
}

fn section_observation_tools() -> String {
    "Observation tools:\n\
      After you navigate, click, fill a form, or refresh a page, use observation tools to understand what happened. These tools expose browser DevTools capabilities: network requests, console logs, WebSocket messages, performance metrics, cookies, and storage.\n\n\
      Key concepts:\n\
      - **Seq numbers**: Every action tool (navigate, click, fill_form, etc.) returns a `seq: N` number. Use this in `since` parameters to filter observations to a specific point in time.\n\
      - **Since/until filtering**: `since: \"last\"` (default) = since your last action; `since: N` = since seq number N; `since: \"all\"` = entire session. `until: N` = exclusive upper bound. Uses half-open interval [since, until).\n\
      - **Overview/detail pattern**: `list_*` tools give compact summaries with @rN/@logN/@wsN IDs. Use the corresponding `inspect_*` tools with those IDs for full details.\n\
      - **Temporal scoping**: Observations are buffered per browser session. Older entries are pruned to keep memory bounded. Use `since=\"all\"` to get the full retained buffer.\n\n\
      Available observation tools:\n\
      - **`list_network_activity`** — List buffered HTTP requests with optional filtering by state (xhr, failed, pending), URL pattern, and sorting (slowest, fastest, largest, smallest, newest, oldest). Returns @rN IDs.\n\
      - **`inspect_request`** — Inspect a request by @rN ID. Returns metadata, timing, initiator type, and notes about unavailable headers/bodies.\n\
      - **`list_page_logs`** — List console logs (error, warning, info, debug) grouped by message text (default), source, or level. Returns @logN IDs for deduplicated groups.\n\
      - **`inspect_log`** — Inspect a log group by @logN ID. Returns concrete instances with timestamps, stack traces, and source locations.\n\
      - **`list_websocket_activity`** — Overview of WebSocket connections with message counts. Returns @wsN IDs.\n\
      - **`inspect_websocket`** — Inspect WebSocket messages by @wsN ID. Supports direction filter (sent/received), pattern search, and sorting.\n\
      - **`get_page_performance`** — Navigation Timing and Resource Timing metrics. Returns TTFB, DOM timings, and top 20 resources by transfer size.\n\
      - **`inspect_cookies`** — All cookies on the current page with security analysis (missing_secure, missing_httponly, excessive_lifetime, etc.).\n\
      - **`inspect_storage`** — LocalStorage and SessionStorage contents.\n\
      - **`measure_coverage`** — CSS and JavaScript coverage metrics.\n\
      - **`audit_accessibility`** — Accessibility audit results (WCAG violations, missing labels, etc.).\n\
      - **`intercept_network`** — Set up request/response interception rules for future requests.\n\n\
      When to use:\n\
      - After navigate/click/fill_form when you want to understand what happened (API calls, errors, performance).\n\
      - When debugging a page that seems broken or unresponsive — check console logs and network requests.\n\
      - When a form submission or API call fails — inspect network activity to see the actual request/response.\n\
      - When performance is slow — use get_page_performance to identify bottlenecks.\n\n\
      Example workflow:\n\
      1. Click a button that triggers an API call.\n\
      2. Call `list_network_activity` with `since=\"last\"` to see requests since the click.\n\
      3. If you see a failed request, call `inspect_request` with its @rN ID to get details.\n\
      4. If you see console errors, call `list_page_logs` with `level=\"error\"` and `since=\"last\"`.\n\
      5. Call `inspect_log` on the @logN ID to see the full stack trace."
        .to_string()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn sample_specs() -> Vec<ToolSpec> {
        vec![
            ToolSpec {
                name: "navigate",
                description: "Navigate to a URL",
                input_schema: json!({
                    "type": "object",
                    "properties": { "url": { "type": "string" } },
                    "required": ["url"]
                }),
                instructions: Some("Always use full URLs."),
            },
            ToolSpec {
                name: "click",
                description: "Click an element",
                input_schema: json!({
                    "type": "object",
                    "properties": { "selector": { "type": "string" } },
                    "required": ["selector"]
                }),
                instructions: None,
            },
            ToolSpec {
                name: "screenshot",
                description: "Take a screenshot",
                input_schema: json!({
                    "type": "object",
                    "properties": {}
                }),
                instructions: None,
            },
        ]
    }

    #[test]
    fn build_system_prompt_includes_tool_listing() {
        let prompt = build_system_prompt(&sample_specs(), None);
        assert!(prompt.len() >= 2, "should have at least 2 prompt sections");

        let first = &prompt[0];
        assert!(
            first.contains("autonomous web crawler"),
            "should identify as crawler agent"
        );
        assert!(
            first.contains("user's web task"),
            "should define mission clearly"
        );
        assert!(first.contains("navigate"), "should list navigate tool");
        assert!(first.contains("click"), "should list click tool");
        assert!(first.contains("screenshot"), "should list screenshot tool");
        assert!(
            first.contains("Always use full URLs."),
            "should render per-tool instructions"
        );
    }

    #[test]
    fn format_tool_shows_params_when_required() {
        let spec = ToolSpec {
            name: "navigate",
            description: "Navigate to a URL",
            input_schema: json!({"required": ["url"]}),
            instructions: None,
        };
        let line = format_tool(&spec);
        assert!(line.contains("params: url"));
    }

    #[test]
    fn format_tool_omits_params_when_none_required() {
        let spec = ToolSpec {
            name: "screenshot",
            description: "Take a screenshot",
            input_schema: json!({"type": "object"}),
            instructions: None,
        };
        let line = format_tool(&spec);
        assert!(!line.contains("params"));
    }

    #[test]
    fn build_system_prompt_contains_all_sections() {
        let prompt = build_system_prompt(&sample_specs(), None);
        let joined = prompt.join("\n");
        assert!(joined.contains("Operating procedure:"));
        assert!(joined.contains("Data integrity:"));
        assert!(joined.contains("Constraints:"));
        assert!(joined.contains("Error recovery by situation:"));
        assert!(joined.contains("Completion:"));
        assert!(joined.contains("Parallel exploration:"));
        assert!(joined.contains("Autonomous scripts:"));
        assert!(joined.contains("Observation tools:"));
    }

    #[test]
    fn build_system_prompt_lists_all_tools() {
        let specs = crate::mvp_tool_specs();
        let prompt = build_system_prompt(&specs, None);
        let first = &prompt[0];
        for spec in &specs {
            assert!(
                first.contains(spec.name),
                "prompt should list tool: {}",
                spec.name
            );
        }
    }

    #[test]
    fn test_system_prompt_contains_parallel_exploration() {
        let specs = crate::mvp_tool_specs();
        let prompt = build_system_prompt(&specs, None);
        let joined = prompt.join("\n");
        assert!(joined.contains("fork"), "should mention fork tool");
        assert!(joined.contains("parallel"), "should mention parallel");
        assert!(joined.contains("subagent"), "should mention subagent");
        assert!(
            joined.contains("wait_for_subagents"),
            "should mention wait_for_subagents"
        );
        assert_eq!(prompt.len(), 9, "should have 9 sections");
    }

    #[test]
    fn test_system_prompt_contains_autonomous_scripts() {
        let specs = crate::mvp_tool_specs();
        let prompt = build_system_prompt(&specs, None);
        let joined = prompt.join("\n");
        assert!(
            joined.contains("Autonomous scripts:"),
            "should mention autonomous scripts"
        );
        assert!(joined.contains("run_script"), "should mention run_script");
        assert!(
            joined.contains("script_status"),
            "should mention script_status"
        );
        assert!(
            joined.contains("wait_for_scripts"),
            "should mention wait_for_scripts"
        );
        assert!(
            joined.contains("cancel_script"),
            "should mention cancel_script"
        );
        assert!(joined.contains("save_script"), "should mention save_script");
        assert!(
            joined.contains("list_scripts"),
            "should mention list_scripts"
        );
        assert!(joined.contains("read_script"), "should mention read_script");
        assert_eq!(prompt.len(), 9, "should have 9 sections");
    }

    #[test]
    fn build_system_prompt_is_unchanged_when_dynamic_context_is_none() {
        let specs = sample_specs();
        let prompt = build_system_prompt(&specs, None);

        assert_eq!(
            prompt,
            vec![
                section_identity(&specs),
                section_operating_procedure(),
                section_data_integrity(),
                section_constraints(),
                section_error_recovery(),
                section_completion(),
                section_parallel_exploration(),
                section_autonomous_scripts(),
                section_observation_tools(),
            ]
        );
    }

    #[test]
    fn build_system_prompt_appends_dynamic_section_when_present() {
        let specs = sample_specs();
        let prompt = build_system_prompt(
            &specs,
            Some(&DynamicPromptContext {
                stagnation_alert: Some("You are stuck".to_string()),
                ..DynamicPromptContext::default()
            }),
        );

        assert_eq!(prompt.len(), 10);
        assert!(prompt[9].contains("You are stuck"));
    }

    #[test]
    fn prompt_contains_recaptcha_v3_remedy() {
        let specs = crate::mvp_tool_specs();
        let prompt = build_system_prompt(&specs, None);
        let joined = prompt.join("\n");

        assert_eq!(
            prompt.len(),
            9,
            "guidance appended inside error-recovery must not add a 10th section"
        );

        assert!(
            joined.contains("reCAPTCHA v3"),
            "should mention reCAPTCHA v3"
        );
        assert!(
            joined.contains("recaptcha_detected: true is NOT a blocker"),
            "recaptcha_detected: true alone must not be treated as a blocker"
        );
        assert!(
            joined.contains("headless"),
            "should explain the headless low-score remedy"
        );
        assert!(
            joined.contains("acrawl config set headless false"),
            "should surface the human-runnable remedy command"
        );
        assert!(
            joined.contains("/extension"),
            "should mention the extension bridge alternative"
        );
    }
}
