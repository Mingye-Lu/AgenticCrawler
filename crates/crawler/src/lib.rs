use runtime::PermissionMode;

/// Specification for a single tool that the agent can invoke.
pub struct ToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: serde_json::Value,
    pub required_permission: PermissionMode,
}

/// Returns the built-in tool specifications.
///
/// Placeholder — will be populated by subsequent tasks.
#[must_use]
pub fn mvp_tool_specs() -> Vec<ToolSpec> {
    Vec::new()
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
