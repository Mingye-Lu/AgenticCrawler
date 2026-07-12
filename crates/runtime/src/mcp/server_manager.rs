use std::collections::BTreeMap;
use std::io;

use serde_json::Value as JsonValue;
use tokio::time::{error::Elapsed, timeout, Duration};

use crate::config::{McpServerConfig, McpTransport, RuntimeConfig};

use super::client::McpClientBootstrap;
use super::naming::mcp_tool_name;
use super::process::{default_initialize_params, spawn_mcp_stdio_process, McpStdioProcess};
use super::types::{
    JsonRpcId, JsonRpcResponse, ManagedMcpTool, McpListToolsParams, McpServerManagerError,
    McpToolCallParams, McpToolCallResult, UnsupportedMcpServer,
};

const MCP_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq, Eq)]
struct ToolRoute {
    server_name: String,
    raw_name: String,
}

#[derive(Debug)]
struct ManagedMcpServer {
    bootstrap: McpClientBootstrap,
    process: Option<McpStdioProcess>,
    initialized: bool,
}

impl ManagedMcpServer {
    fn new(bootstrap: McpClientBootstrap) -> Self {
        Self {
            bootstrap,
            process: None,
            initialized: false,
        }
    }
}

#[derive(Debug)]
pub struct McpServerManager {
    servers: BTreeMap<String, ManagedMcpServer>,
    unsupported_servers: Vec<UnsupportedMcpServer>,
    tool_index: BTreeMap<String, ToolRoute>,
    next_request_id: u64,
}

impl McpServerManager {
    #[must_use]
    pub fn from_runtime_config(config: &RuntimeConfig) -> Self {
        Self::from_servers(config.mcp().servers())
    }

    #[must_use]
    pub fn from_servers(servers: &BTreeMap<String, McpServerConfig>) -> Self {
        let mut managed_servers = BTreeMap::new();
        let mut unsupported_servers = Vec::new();

        for (server_name, server_config) in servers {
            if server_config.transport() == McpTransport::Stdio {
                let bootstrap = McpClientBootstrap::from_config(server_name, server_config);
                managed_servers.insert(server_name.clone(), ManagedMcpServer::new(bootstrap));
            } else {
                unsupported_servers.push(UnsupportedMcpServer {
                    server_name: server_name.clone(),
                    transport: server_config.transport(),
                    reason: format!(
                        "transport {:?} is not supported by McpServerManager",
                        server_config.transport()
                    ),
                });
            }
        }

        Self {
            servers: managed_servers,
            unsupported_servers,
            tool_index: BTreeMap::new(),
            next_request_id: 1,
        }
    }

    #[must_use]
    pub fn unsupported_servers(&self) -> &[UnsupportedMcpServer] {
        &self.unsupported_servers
    }

    pub async fn discover_tools(&mut self) -> Result<Vec<ManagedMcpTool>, McpServerManagerError> {
        let server_names = self.servers.keys().cloned().collect::<Vec<_>>();
        let mut discovered_tools = Vec::new();

        for server_name in server_names {
            self.ensure_server_ready(&server_name).await?;
            self.clear_routes_for_server(&server_name);

            let mut cursor = None;
            loop {
                let request_id = self.take_request_id();
                let outcome = {
                    let server = self.server_mut(&server_name)?;
                    let process = server.process.as_mut().ok_or_else(|| {
                        McpServerManagerError::InvalidResponse {
                            server_name: server_name.clone(),
                            method: "tools/list",
                            details:
                                "MCP server process is gone — it likely crashed or was killed; \
                                      check the server's stderr output above and retry"
                                    .to_string(),
                        }
                    })?;
                    timeout(
                        MCP_REQUEST_TIMEOUT,
                        process.list_tools(
                            request_id,
                            Some(McpListToolsParams {
                                cursor: cursor.clone(),
                            }),
                        ),
                    )
                    .await
                };
                let response = self.finish_timeout(&server_name, "tools/list", outcome)?;

                if let Some(error) = response.error {
                    return Err(McpServerManagerError::JsonRpc {
                        server_name: server_name.clone(),
                        method: "tools/list",
                        error,
                    });
                }

                let result =
                    response
                        .result
                        .ok_or_else(|| McpServerManagerError::InvalidResponse {
                            server_name: server_name.clone(),
                            method: "tools/list",
                            details: "missing result payload".to_string(),
                        })?;

                for tool in result.tools {
                    let qualified_name = mcp_tool_name(&server_name, &tool.name);
                    self.tool_index.insert(
                        qualified_name.clone(),
                        ToolRoute {
                            server_name: server_name.clone(),
                            raw_name: tool.name.clone(),
                        },
                    );
                    discovered_tools.push(ManagedMcpTool {
                        server_name: server_name.clone(),
                        qualified_name,
                        raw_name: tool.name.clone(),
                        tool,
                    });
                }

                match result.next_cursor {
                    Some(next_cursor) => cursor = Some(next_cursor),
                    None => break,
                }
            }
        }

        Ok(discovered_tools)
    }

    pub async fn call_tool(
        &mut self,
        qualified_tool_name: &str,
        arguments: Option<JsonValue>,
    ) -> Result<JsonRpcResponse<McpToolCallResult>, McpServerManagerError> {
        let route = self
            .tool_index
            .get(qualified_tool_name)
            .cloned()
            .ok_or_else(|| McpServerManagerError::UnknownTool {
                qualified_name: qualified_tool_name.to_string(),
            })?;

        self.ensure_server_ready(&route.server_name).await?;
        let request_id = self.take_request_id();
        let outcome =
            {
                let server = self.server_mut(&route.server_name)?;
                let process = server.process.as_mut().ok_or_else(|| {
                    McpServerManagerError::InvalidResponse {
                        server_name: route.server_name.clone(),
                        method: "tools/call",
                        details: "server process missing after initialization".to_string(),
                    }
                })?;
                timeout(
                    MCP_REQUEST_TIMEOUT,
                    process.call_tool(
                        request_id,
                        McpToolCallParams {
                            name: route.raw_name,
                            arguments,
                            meta: None,
                        },
                    ),
                )
                .await
            };
        let response = self.finish_timeout(&route.server_name, "tools/call", outcome)?;
        Ok(response)
    }

    pub async fn shutdown(&mut self) -> Result<(), McpServerManagerError> {
        let server_names = self.servers.keys().cloned().collect::<Vec<_>>();
        for server_name in server_names {
            let server = self.server_mut(&server_name)?;
            if let Some(process) = server.process.as_mut() {
                process.shutdown().await?;
            }
            server.process = None;
            server.initialized = false;
        }
        Ok(())
    }

    fn clear_routes_for_server(&mut self, server_name: &str) {
        self.tool_index
            .retain(|_, route| route.server_name != server_name);
    }

    fn server_mut(
        &mut self,
        server_name: &str,
    ) -> Result<&mut ManagedMcpServer, McpServerManagerError> {
        self.servers
            .get_mut(server_name)
            .ok_or_else(|| McpServerManagerError::UnknownServer {
                server_name: server_name.to_string(),
            })
    }

    fn take_request_id(&mut self) -> JsonRpcId {
        // saturating_add would have pinned every subsequent request at
        // u64::MAX once the counter reached the ceiling, repeatedly issuing
        // the same id and breaking JSON-RPC correlation. Wrap to 1 instead;
        // any in-flight requests will have resolved long before another
        // 2^64 ids are minted, so reuse-after-wrap is benign.
        let id = self.next_request_id;
        self.next_request_id = self.next_request_id.checked_add(1).unwrap_or(1);
        JsonRpcId::Number(id)
    }

    /// Drop a server's process/connection state after an unrecoverable
    /// failure (currently: a request timeout). The request bytes for a
    /// timed-out call were already written to the child's stdin, so the
    /// child may still write a response for it later. Rather than risk a
    /// later, unrelated call reading that stale frame off the same stream,
    /// kill the process (`McpStdioProcess`'s `Drop` impl kills the child)
    /// and mark the server as needing a fresh spawn + handshake on its next
    /// use.
    fn invalidate_server(&mut self, server_name: &str) {
        if let Some(server) = self.servers.get_mut(server_name) {
            server.process = None;
            server.initialized = false;
        }
    }

    /// Resolve the result of a `timeout(...).await` call. On success,
    /// unwraps the inner `io::Result`. On timeout, invalidates the server's
    /// connection (see [`Self::invalidate_server`]) and returns
    /// [`McpServerManagerError::Timeout`].
    fn finish_timeout<T>(
        &mut self,
        server_name: &str,
        method: &'static str,
        outcome: Result<io::Result<T>, Elapsed>,
    ) -> Result<T, McpServerManagerError> {
        match outcome {
            Ok(inner) => Ok(inner?),
            Err(_elapsed) => {
                self.invalidate_server(server_name);
                Err(McpServerManagerError::Timeout {
                    server_name: server_name.to_string(),
                    method,
                    timeout: MCP_REQUEST_TIMEOUT,
                })
            }
        }
    }

    async fn ensure_server_ready(
        &mut self,
        server_name: &str,
    ) -> Result<(), McpServerManagerError> {
        let needs_spawn = self
            .servers
            .get(server_name)
            .map(|server| server.process.is_none())
            .ok_or_else(|| McpServerManagerError::UnknownServer {
                server_name: server_name.to_string(),
            })?;

        if needs_spawn {
            let server = self.server_mut(server_name)?;
            server.process = Some(spawn_mcp_stdio_process(&server.bootstrap)?);
            server.initialized = false;
        }

        let needs_initialize = self
            .servers
            .get(server_name)
            .map(|server| !server.initialized)
            .ok_or_else(|| McpServerManagerError::UnknownServer {
                server_name: server_name.to_string(),
            })?;

        if needs_initialize {
            let request_id = self.take_request_id();
            let outcome = {
                let server = self.server_mut(server_name)?;
                let process = server.process.as_mut().ok_or_else(|| {
                    McpServerManagerError::InvalidResponse {
                        server_name: server_name.to_string(),
                        method: "initialize",
                        details: "MCP server process is gone before initialize — \
                                  spawn appears to have failed silently; \
                                  check the server's stderr output above and retry"
                            .to_string(),
                    }
                })?;
                timeout(
                    MCP_REQUEST_TIMEOUT,
                    process.initialize(request_id, default_initialize_params()),
                )
                .await
            };
            let response = self.finish_timeout(server_name, "initialize", outcome)?;

            if let Some(error) = response.error {
                return Err(McpServerManagerError::JsonRpc {
                    server_name: server_name.to_string(),
                    method: "initialize",
                    error,
                });
            }

            if response.result.is_none() {
                return Err(McpServerManagerError::InvalidResponse {
                    server_name: server_name.to_string(),
                    method: "initialize",
                    details: "missing result payload".to_string(),
                });
            }

            {
                let outcome = {
                    let server = self.server_mut(server_name)?;
                    let process = server.process.as_mut().ok_or_else(|| {
                        McpServerManagerError::InvalidResponse {
                            server_name: server_name.to_string(),
                            method: "notifications/initialized",
                            details: "MCP server process is gone after initialize but before \
                                  the initialized notification — the server probably exited mid-handshake; \
                                  check the server's stderr output above and retry"
                                .to_string(),
                        }
                    })?;
                    timeout(MCP_REQUEST_TIMEOUT, process.notify_initialized()).await
                };
                self.finish_timeout(server_name, "notifications/initialized", outcome)?;
            }

            let server = self.server_mut(server_name)?;
            server.initialized = true;
        }

        Ok(())
    }
}

#[cfg(all(test, unix))]
#[path = "server_manager_tests.rs"]
mod tests;
