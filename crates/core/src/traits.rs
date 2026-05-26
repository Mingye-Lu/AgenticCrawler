use crate::{error::ToolError, ToolOutcome};

#[allow(async_fn_in_trait)]
pub trait ToolExecutor {
    async fn execute(&mut self, tool_name: &str, input: &str) -> Result<ToolOutcome, ToolError>;
}
