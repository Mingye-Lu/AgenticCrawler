use std::future::Future;

use crate::{error::ToolError, ToolOutcome};

pub trait ToolExecutor {
    fn execute(
        &mut self,
        tool_name: &str,
        input: &str,
    ) -> impl Future<Output = Result<ToolOutcome, ToolError>> + Send;
}
