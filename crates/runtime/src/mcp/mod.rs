mod client;
mod naming;
mod process;
mod server_manager;
mod types;

// Re-export internally for crate tests that use `crate::mcp::X` paths
#[cfg(test)]
pub(crate) use client::McpClientBootstrap;

pub use naming::{mcp_tool_name, mcp_tool_prefix};
pub use process::{encode_mcp_frame, read_mcp_frame};
pub use server_manager::McpServerManager;
pub use types::{
    JsonRpcError, JsonRpcId, JsonRpcResponse, ManagedMcpTool, McpServerManagerError, McpTool,
    McpToolCallContent, McpToolCallParams, McpToolCallResult, UnsupportedMcpServer,
};
