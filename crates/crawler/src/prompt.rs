use crate::ToolSpec;

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
pub fn build_system_prompt(tool_specs: &[ToolSpec]) -> Vec<String> {
    let tool_listing = list_tools(tool_specs);

    vec![
        // Section 1 — identity + tool listing
        format!(
            "You are an autonomous web crawler agent. Your job is to complete the \
             user's web task by navigating websites, interacting with pages, and \
             extracting information from page content only.\n\n\
             Your task arrives as a user message. Read it carefully before acting.\n\n\
             Available browser tools:\n{tool_listing}"
        ),
        // Section 2 — operating procedure (replaces vague "think step by step")
        "Operating procedure:\n\
         1. Read the current task carefully. Identify every concrete requirement.\n\
         2. Start from the most relevant direct URL when known, using a full URL \
         including https://.\n\
         3. At each step:\n\
         \x20\x20 a. Observe the current page state from the last tool result.\n\
         \x20\x20 b. Decide the single best next action.\n\
         \x20\x20 c. Execute one tool call.\n\
         \x20\x20 d. Evaluate the result before continuing.\n\
         4. Prefer the simplest reliable action:\n\
         \x20\x20 - Direct navigate over clicking links when the URL is known.\n\
         \x20\x20 - click, fill_form, and scroll before execute_js.\n\
         \x20\x20 - extract_data over free-form summarization when data is requested.\n\
         5. Use extract_data whenever the task requires collecting information. \
         Output valid JSON following the requested schema.\n\
         6. When you have accomplished the goal, provide a clear summary of what \
         was found and any structured data extracted."
            .to_string(),
        // Section 3 — data integrity (anti-hallucination + grounding)
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
            .to_string(),
        // Section 4 — constraints (concrete budgets, not vague advice)
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
            .to_string(),
        // Section 5 — error recovery (tiered by situation, not generic)
        "Error recovery by situation:\n\
         - Selector not found: Try a more general selector, scroll to reveal \
         content, or check for overlays or popups blocking the element.\n\
         - Page not loading or navigation error: Verify the URL is correct and \
         complete. Try an alternative URL or search for the page.\n\
         - Login wall or paywall: Stop immediately. Report the blocker and provide \
         any partial results collected so far.\n\
         - Anti-bot detection or CAPTCHA: Wait briefly and retry once. If it \
         persists, stop and report the blocker.\n\
         - Empty results on a page expected to have data: Scroll down for \
         lazy-loaded content, wait for dynamic rendering, or check whether the \
         page uses iframes.\n\
         - JavaScript interaction failing: Use execute_js as a fallback when \
         click or fill_form fail, but not as a first choice.\n\
         - Popup or overlay blocking interaction: Try pressing Escape or clicking \
         a dismiss button before retrying the intended action."
            .to_string(),
        // Section 6 — completion protocol
         "Completion:\n\
          - Before reporting success, re-read the original task and confirm each \
          requirement is met by data you actually extracted.\n\
          - If any requirement is unmet or uncertain, say so explicitly rather \
          than guessing.\n\
          - If the task is impossible to continue (requires login, payment, or \
          access you do not have), stop immediately and explain why.\n\
          - When providing extracted data, include the source URL and note any \
          gaps or limitations."
             .to_string(),
         // Section 7 — parallel exploration
         "Parallel exploration:\n\
          - Use the `fork` tool to spawn a subagent on a separate browser tab when you need \
          to explore multiple pages simultaneously.\n\
          - Each subagent gets a copy of your history and works independently.\n\
          - You can fork multiple subagents at once (up to the configured limit).\n\
          - After forking, continue your own work — subagents run in parallel.\n\
          - Use `wait_for_subagents` to pause and collect results when you need them.\n\
          - When you call `done`, the system automatically waits for all subagents and merges their data.\n\
          - Prefer forking over sequential navigation when visiting multiple independent pages.\n\
          - Example: Scraping 5 product pages? Fork 5 subagents, each visiting one page, then call done."
             .to_string(),
     ]
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
        let prompt = build_system_prompt(&sample_specs());
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
         let prompt = build_system_prompt(&sample_specs());
         let joined = prompt.join("\n");
         assert!(joined.contains("Operating procedure:"));
         assert!(joined.contains("Data integrity:"));
         assert!(joined.contains("Constraints:"));
         assert!(joined.contains("Error recovery by situation:"));
         assert!(joined.contains("Completion:"));
         assert!(joined.contains("Parallel exploration:"));
     }

     #[test]
     fn build_system_prompt_lists_all_18_tools() {
         let specs = crate::mvp_tool_specs();
         let prompt = build_system_prompt(&specs);
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
         let prompt = build_system_prompt(&specs);
         let joined = prompt.join("\n");
         assert!(joined.contains("fork"), "should mention fork tool");
         assert!(joined.contains("parallel"), "should mention parallel");
         assert!(joined.contains("subagent"), "should mention subagent");
         assert!(joined.contains("wait_for_subagents"), "should mention wait_for_subagents");
         assert_eq!(prompt.len(), 7, "should have 7 sections");
     }
 }
