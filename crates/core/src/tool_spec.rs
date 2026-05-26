/// Specification for a single tool that the agent can invoke.
pub struct ToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: serde_json::Value,
    /// Extended usage guidance rendered into the system prompt.
    pub instructions: Option<&'static str>,
}
