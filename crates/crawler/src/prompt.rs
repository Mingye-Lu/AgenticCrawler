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
        format!(
            "You are an autonomous web crawler agent. Your goal is to navigate websites, \
             extract data, and interact with web pages to accomplish tasks.\n\n\
             You have access to the following browser tools:\n{tool_listing}"
        ),
        "Instructions:\n\
         - Think step by step about how to achieve the goal.\n\
         - Always start by navigating to the relevant URL using full URLs (including https://).\n\
         - Use tools methodically to interact with pages: click buttons, fill forms, scroll, \
         and extract data as needed.\n\
         - When extracting data, return it as structured JSON via the extract_data tool.\n\
         - If a page requires JavaScript interaction, use click, fill_form, execute_js, \
         and other interaction tools.\n\
         - When you have accomplished the goal, provide a clear summary of what you found."
            .to_string(),
        "Constraints:\n\
         - Do NOT loop indefinitely. If you cannot make progress after several attempts, \
         summarize what you found and stop.\n\
         - Keep extracted data clean and well-structured.\n\
         - Prefer navigate with a direct URL over clicking links when possible.\n\
         - Use go_back to return to previous pages instead of re-navigating."
            .to_string(),
        "Error recovery:\n\
         1. Retry with a different CSS selector.\n\
         2. Try a different URL or page on the same site.\n\
         3. Try a different search engine or query.\n\
         4. Stop with whatever partial results you have and explain the blocker."
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
    fn build_system_prompt_contains_instructions_and_constraints() {
        let prompt = build_system_prompt(&sample_specs());
        let joined = prompt.join("\n");
        assert!(joined.contains("Instructions:"));
        assert!(joined.contains("Constraints:"));
        assert!(joined.contains("Error recovery:"));
    }

    #[test]
    fn build_system_prompt_lists_all_15_tools() {
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
}
