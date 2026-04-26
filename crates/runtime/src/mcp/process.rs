use std::collections::BTreeMap;
use std::io;
use std::process::Stdio;

use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value as JsonValue;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

use super::client::{McpClientBootstrap, McpClientTransport, McpStdioTransport};
use super::types::{
    JsonRpcId, JsonRpcNotification, JsonRpcRequest, JsonRpcResponse, McpInitializeClientInfo,
    McpInitializeParams, McpInitializeResult, McpListResourcesParams, McpListResourcesResult,
    McpListToolsParams, McpListToolsResult, McpReadResourceParams, McpReadResourceResult,
    McpToolCallParams, McpToolCallResult,
};

#[derive(Debug)]
pub struct McpStdioProcess {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl McpStdioProcess {
    pub fn spawn(transport: &McpStdioTransport) -> io::Result<Self> {
        let mut command = Command::new(&transport.command);
        command
            .args(&transport.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        apply_env(&mut command, &transport.env);

        let mut child = command.spawn()?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("stdio MCP process missing stdin pipe"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("stdio MCP process missing stdout pipe"))?;

        Ok(Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
        })
    }

    pub async fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.stdin.write_all(bytes).await
    }

    pub async fn flush(&mut self) -> io::Result<()> {
        self.stdin.flush().await
    }

    pub async fn write_line(&mut self, line: &str) -> io::Result<()> {
        self.write_all(line.as_bytes()).await?;
        self.write_all(b"\n").await?;
        self.flush().await
    }

    pub async fn read_line(&mut self) -> io::Result<String> {
        let mut line = String::new();
        let bytes_read = self.stdout.read_line(&mut line).await?;
        if bytes_read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "MCP stdio stream closed while reading line",
            ));
        }
        Ok(line)
    }

    pub async fn read_available(&mut self) -> io::Result<Vec<u8>> {
        let mut buffer = vec![0_u8; 4096];
        let read = self.stdout.read(&mut buffer).await?;
        buffer.truncate(read);
        Ok(buffer)
    }

    pub async fn write_frame(&mut self, payload: &[u8]) -> io::Result<()> {
        let encoded = encode_frame(payload);
        self.write_all(&encoded).await?;
        self.flush().await
    }

    pub async fn read_frame(&mut self) -> io::Result<Vec<u8>> {
        let mut content_length = None;
        loop {
            let mut line = String::new();
            let bytes_read = self.stdout.read_line(&mut line).await?;
            if bytes_read == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "MCP stdio stream closed while reading headers",
                ));
            }
            if line == "\r\n" {
                break;
            }
            if let Some(value) = line.strip_prefix("Content-Length:") {
                let parsed = value
                    .trim()
                    .parse::<usize>()
                    .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
                content_length = Some(parsed);
            }
        }

        let content_length = content_length.ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "missing Content-Length header")
        })?;
        let mut payload = vec![0_u8; content_length];
        self.stdout.read_exact(&mut payload).await?;
        Ok(payload)
    }

    pub async fn write_jsonrpc_message<T: Serialize>(&mut self, message: &T) -> io::Result<()> {
        let body = serde_json::to_vec(message)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        self.write_frame(&body).await
    }

    pub async fn read_jsonrpc_message<T: DeserializeOwned>(&mut self) -> io::Result<T> {
        let payload = self.read_frame().await?;
        serde_json::from_slice(&payload)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    }

    pub async fn send_request<T: Serialize>(
        &mut self,
        request: &JsonRpcRequest<T>,
    ) -> io::Result<()> {
        self.write_jsonrpc_message(request).await
    }

    pub async fn send_notification<T: Serialize>(
        &mut self,
        notification: &JsonRpcNotification<T>,
    ) -> io::Result<()> {
        self.write_jsonrpc_message(notification).await
    }

    pub async fn read_response<T: DeserializeOwned>(&mut self) -> io::Result<JsonRpcResponse<T>> {
        self.read_jsonrpc_message().await
    }

    pub async fn request<TParams: Serialize, TResult: DeserializeOwned>(
        &mut self,
        id: JsonRpcId,
        method: impl Into<String>,
        params: Option<TParams>,
    ) -> io::Result<JsonRpcResponse<TResult>> {
        let request = JsonRpcRequest::new(id, method, params);
        self.send_request(&request).await?;
        self.read_response().await
    }

    pub async fn initialize(
        &mut self,
        id: JsonRpcId,
        params: McpInitializeParams,
    ) -> io::Result<JsonRpcResponse<McpInitializeResult>> {
        self.request(id, "initialize", Some(params)).await
    }

    pub async fn notify_initialized(&mut self) -> io::Result<()> {
        let notification = JsonRpcNotification::<JsonValue>::new("notifications/initialized", None);
        self.send_notification(&notification).await
    }

    pub async fn list_tools(
        &mut self,
        id: JsonRpcId,
        params: Option<McpListToolsParams>,
    ) -> io::Result<JsonRpcResponse<McpListToolsResult>> {
        self.request(id, "tools/list", params).await
    }

    pub async fn call_tool(
        &mut self,
        id: JsonRpcId,
        params: McpToolCallParams,
    ) -> io::Result<JsonRpcResponse<McpToolCallResult>> {
        self.request(id, "tools/call", Some(params)).await
    }

    pub async fn list_resources(
        &mut self,
        id: JsonRpcId,
        params: Option<McpListResourcesParams>,
    ) -> io::Result<JsonRpcResponse<McpListResourcesResult>> {
        self.request(id, "resources/list", params).await
    }

    pub async fn read_resource(
        &mut self,
        id: JsonRpcId,
        params: McpReadResourceParams,
    ) -> io::Result<JsonRpcResponse<McpReadResourceResult>> {
        self.request(id, "resources/read", Some(params)).await
    }

    pub async fn terminate(&mut self) -> io::Result<()> {
        self.child.kill().await
    }

    pub async fn wait(&mut self) -> io::Result<std::process::ExitStatus> {
        self.child.wait().await
    }

    pub async fn shutdown(&mut self) -> io::Result<()> {
        if self.child.try_wait()?.is_none() {
            self.child.kill().await?;
        }
        let _ = self.child.wait().await?;
        Ok(())
    }
}

impl Drop for McpStdioProcess {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.start_kill();
        }
    }
}

pub fn spawn_mcp_stdio_process(bootstrap: &McpClientBootstrap) -> io::Result<McpStdioProcess> {
    match &bootstrap.transport {
        McpClientTransport::Stdio(transport) => McpStdioProcess::spawn(transport),
        other => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "MCP bootstrap transport for {} is not stdio: {other:?}",
                bootstrap.server_name
            ),
        )),
    }
}

pub(crate) fn default_initialize_params() -> McpInitializeParams {
    McpInitializeParams {
        protocol_version: "2025-03-26".to_string(),
        capabilities: JsonValue::Object(serde_json::Map::new()),
        client_info: McpInitializeClientInfo {
            name: "runtime".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
        },
    }
}

fn apply_env(command: &mut Command, env: &BTreeMap<String, String>) {
    for (key, value) in env {
        command.env(key, value);
    }
}

fn encode_frame(payload: &[u8]) -> Vec<u8> {
    let header = format!("Content-Length: {}\r\n\r\n", payload.len());
    let mut framed = header.into_bytes();
    framed.extend_from_slice(payload);
    framed
}

#[cfg(all(test, unix))]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::io::ErrorKind;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    use serde_json::json;
    use tokio::runtime::Builder;

    use crate::config::{
        ConfigSource, McpSdkServerConfig, McpServerConfig, McpStdioServerConfig,
        ScopedMcpServerConfig,
    };
    use crate::mcp::McpInitializeServerInfo;

    use super::{
        default_initialize_params, encode_frame, spawn_mcp_stdio_process, JsonRpcId,
        JsonRpcRequest, JsonRpcResponse, McpClientBootstrap, McpInitializeClientInfo,
        McpInitializeParams, McpInitializeResult, McpListToolsResult, McpReadResourceParams,
        McpReadResourceResult, McpStdioProcess, McpToolCallParams,
    };
    use crate::mcp::McpTool;

    fn temp_dir() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("runtime-mcp-stdio-{nanos}"))
    }

    fn write_echo_script() -> PathBuf {
        let root = temp_dir();
        fs::create_dir_all(&root).expect("temp dir");
        let script_path = root.join("echo-mcp.sh");
        fs::write(
            &script_path,
            "#!/bin/sh\nprintf 'READY:%s\\n' \"$MCP_TEST_TOKEN\"\nIFS= read -r line\nprintf 'ECHO:%s\\n' \"$line\"\n",
        )
        .expect("write script");
        let mut permissions = fs::metadata(&script_path).expect("metadata").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&script_path, permissions).expect("chmod");
        script_path
    }

    fn write_jsonrpc_script() -> PathBuf {
        let root = temp_dir();
        fs::create_dir_all(&root).expect("temp dir");
        let script_path = root.join("jsonrpc-mcp.py");
        let script = [
            "#!/usr/bin/env python3",
            "import json, sys",
            "header = b''",
            r"while not header.endswith(b'\r\n\r\n'):",
            "    chunk = sys.stdin.buffer.read(1)",
            "    if not chunk:",
            "        raise SystemExit(1)",
            "    header += chunk",
            "length = 0",
            r"for line in header.decode().split('\r\n'):",
            r"    if line.lower().startswith('content-length:'):",
            r"        length = int(line.split(':', 1)[1].strip())",
            "payload = sys.stdin.buffer.read(length)",
            "request = json.loads(payload.decode())",
            r"assert request['jsonrpc'] == '2.0'",
            r"assert request['method'] == 'initialize'",
            r"response = json.dumps({",
            r"    'jsonrpc': '2.0',",
            r"    'id': request['id'],",
            r"    'result': {",
            r"        'protocolVersion': request['params']['protocolVersion'],",
            r"        'capabilities': {'tools': {}},",
            r"        'serverInfo': {'name': 'fake-mcp', 'version': '0.1.0'}",
            r"    }",
            r"}).encode()",
            r"sys.stdout.buffer.write(f'Content-Length: {len(response)}\r\n\r\n'.encode() + response)",
            "sys.stdout.buffer.flush()",
            "",
        ]
        .join("\n");
        fs::write(&script_path, script).expect("write script");
        let mut permissions = fs::metadata(&script_path).expect("metadata").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&script_path, permissions).expect("chmod");
        script_path
    }

    /// Fake MCP server script that handles initialize, tools/list, tools/call,
    /// resources/list, and resources/read. Notifications (no JSON-RPC `id`) are
    /// silently skipped.
    const FAKE_MCP_SERVER_PY: &str = "\
#!/usr/bin/env python3
import json, sys

def read_message():
    header = b''
    while not header.endswith(b'\\r\\n\\r\\n'):
        chunk = sys.stdin.buffer.read(1)
        if not chunk:
            return None
        header += chunk
    length = 0
    for line in header.decode().split('\\r\\n'):
        if line.lower().startswith('content-length:'):
            length = int(line.split(':', 1)[1].strip())
    payload = sys.stdin.buffer.read(length)
    return json.loads(payload.decode())

def send_message(message):
    payload = json.dumps(message).encode()
    sys.stdout.buffer.write(f'Content-Length: {len(payload)}\\r\\n\\r\\n'.encode() + payload)
    sys.stdout.buffer.flush()

while True:
    request = read_message()
    if request is None:
        break
    method = request['method']
    if 'id' not in request:
        continue
    if method == 'initialize':
        send_message({
            'jsonrpc': '2.0',
            'id': request['id'],
            'result': {
                'protocolVersion': request['params']['protocolVersion'],
                'capabilities': {'tools': {}, 'resources': {}},
                'serverInfo': {'name': 'fake-mcp', 'version': '0.2.0'}
            }
        })
    elif method == 'tools/list':
        send_message({
            'jsonrpc': '2.0',
            'id': request['id'],
            'result': {
                'tools': [
                    {
                        'name': 'echo',
                        'description': 'Echoes text',
                        'inputSchema': {
                            'type': 'object',
                            'properties': {'text': {'type': 'string'}},
                            'required': ['text']
                        }
                    }
                ]
            }
        })
    elif method == 'tools/call':
        args = request['params'].get('arguments') or {}
        if request['params']['name'] == 'fail':
            send_message({
                'jsonrpc': '2.0',
                'id': request['id'],
                'error': {'code': -32001, 'message': 'tool failed'},
            })
        else:
            text = args.get('text', '')
            send_message({
                'jsonrpc': '2.0',
                'id': request['id'],
                'result': {
                    'content': [{'type': 'text', 'text': f'echo:{text}'}],
                    'structuredContent': {'echoed': text},
                    'isError': False
                }
            })
    elif method == 'resources/list':
        send_message({
            'jsonrpc': '2.0',
            'id': request['id'],
            'result': {
                'resources': [
                    {
                        'uri': 'file://guide.txt',
                        'name': 'guide',
                        'description': 'Guide text',
                        'mimeType': 'text/plain'
                    }
                ]
            }
        })
    elif method == 'resources/read':
        uri = request['params']['uri']
        send_message({
            'jsonrpc': '2.0',
            'id': request['id'],
            'result': {
                'contents': [
                    {
                        'uri': uri,
                        'mimeType': 'text/plain',
                        'text': f'contents for {uri}'
                    }
                ]
            }
        })
    else:
        send_message({
            'jsonrpc': '2.0',
            'id': request['id'],
            'error': {'code': -32601, 'message': f'unknown method: {method}'},
        })
";

    fn write_mcp_server_script() -> PathBuf {
        let root = temp_dir();
        fs::create_dir_all(&root).expect("temp dir");
        let script_path = root.join("fake-mcp-server.py");
        fs::write(&script_path, FAKE_MCP_SERVER_PY).expect("write script");
        let mut permissions = fs::metadata(&script_path).expect("metadata").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&script_path, permissions).expect("chmod");
        script_path
    }

    fn write_missing_header_script() -> PathBuf {
        let root = temp_dir();
        fs::create_dir_all(&root).expect("temp dir");
        let script_path = root.join("missing-header.py");
        let script = [
            "#!/usr/bin/env python3",
            "import sys",
            "sys.stdout.buffer.write(b'X-Test: 1\\r\\n\\r\\n{}')",
            "sys.stdout.buffer.flush()",
            "",
        ]
        .join("\n");
        fs::write(&script_path, script).expect("write script");
        let mut permissions = fs::metadata(&script_path).expect("metadata").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&script_path, permissions).expect("chmod");
        script_path
    }

    fn sample_bootstrap(script_path: &Path) -> McpClientBootstrap {
        let config = ScopedMcpServerConfig {
            scope: ConfigSource::Local,
            config: McpServerConfig::Stdio(McpStdioServerConfig {
                command: "/bin/sh".to_string(),
                args: vec![script_path.to_string_lossy().into_owned()],
                env: BTreeMap::from([("MCP_TEST_TOKEN".to_string(), "secret-value".to_string())]),
            }),
        };
        McpClientBootstrap::from_scoped_config("stdio server", &config)
    }

    fn script_transport(script_path: &Path) -> crate::mcp::client::McpStdioTransport {
        crate::mcp::client::McpStdioTransport {
            command: "python3".to_string(),
            args: vec![script_path.to_string_lossy().into_owned()],
            env: BTreeMap::new(),
        }
    }

    fn cleanup_script(script_path: &Path) {
        fs::remove_file(script_path).expect("cleanup script");
        fs::remove_dir_all(script_path.parent().expect("script parent")).expect("cleanup dir");
    }

    #[test]
    fn spawns_stdio_process_and_round_trips_io() {
        let runtime = Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        runtime.block_on(async {
            let script_path = write_echo_script();
            let bootstrap = sample_bootstrap(&script_path);
            let mut process = spawn_mcp_stdio_process(&bootstrap).expect("spawn stdio process");

            let ready = process.read_line().await.expect("read ready");
            assert_eq!(ready, "READY:secret-value\n");

            process
                .write_line("ping from client")
                .await
                .expect("write line");

            let echoed = process.read_line().await.expect("read echo");
            assert_eq!(echoed, "ECHO:ping from client\n");

            let status = process.wait().await.expect("wait for exit");
            assert!(status.success());

            cleanup_script(&script_path);
        });
    }

    #[test]
    fn rejects_non_stdio_bootstrap() {
        let config = ScopedMcpServerConfig {
            scope: ConfigSource::Local,
            config: McpServerConfig::Sdk(McpSdkServerConfig {
                name: "sdk-server".to_string(),
            }),
        };
        let bootstrap = McpClientBootstrap::from_scoped_config("sdk server", &config);
        let error = spawn_mcp_stdio_process(&bootstrap).expect_err("non-stdio should fail");
        assert_eq!(error.kind(), ErrorKind::InvalidInput);
    }

    #[test]
    fn round_trips_initialize_request_and_response_over_stdio_frames() {
        let runtime = Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        runtime.block_on(async {
            let script_path = write_jsonrpc_script();
            let transport = script_transport(&script_path);
            let mut process = McpStdioProcess::spawn(&transport).expect("spawn transport directly");

            let response = process
                .initialize(
                    JsonRpcId::Number(1),
                    McpInitializeParams {
                        protocol_version: "2025-03-26".to_string(),
                        capabilities: json!({"roots": {}}),
                        client_info: McpInitializeClientInfo {
                            name: "runtime-tests".to_string(),
                            version: "0.1.0".to_string(),
                        },
                    },
                )
                .await
                .expect("initialize roundtrip");

            assert_eq!(response.id, JsonRpcId::Number(1));
            assert_eq!(response.error, None);
            assert_eq!(
                response.result,
                Some(McpInitializeResult {
                    protocol_version: "2025-03-26".to_string(),
                    capabilities: json!({"tools": {}}),
                    server_info: McpInitializeServerInfo {
                        name: "fake-mcp".to_string(),
                        version: "0.1.0".to_string(),
                    },
                })
            );

            let status = process.wait().await.expect("wait for exit");
            assert!(status.success());

            cleanup_script(&script_path);
        });
    }

    #[test]
    fn test_json_rpc_request_format() {
        let runtime = Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        runtime.block_on(async {
            let script_path = write_jsonrpc_script();
            let transport = script_transport(&script_path);
            let mut process = McpStdioProcess::spawn(&transport).expect("spawn transport directly");
            let request = JsonRpcRequest::new(
                JsonRpcId::Number(7),
                "initialize",
                Some(json!({
                    "protocolVersion": "2025-03-26",
                    "capabilities": {},
                    "clientInfo": {"name": "runtime-tests", "version": "0.1.0"}
                })),
            );

            process.send_request(&request).await.expect("send request");
            let response: JsonRpcResponse<serde_json::Value> =
                process.read_response().await.expect("read response");

            assert_eq!(response.id, JsonRpcId::Number(7));
            assert_eq!(response.jsonrpc, "2.0");

            let status = process.wait().await.expect("wait for exit");
            assert!(status.success());

            cleanup_script(&script_path);
        });
    }

    #[test]
    fn direct_spawn_uses_transport_env() {
        let runtime = Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        runtime.block_on(async {
            let script_path = write_echo_script();
            let transport = crate::mcp::client::McpStdioTransport {
                command: "/bin/sh".to_string(),
                args: vec![script_path.to_string_lossy().into_owned()],
                env: BTreeMap::from([("MCP_TEST_TOKEN".to_string(), "direct-secret".to_string())]),
            };
            let mut process = McpStdioProcess::spawn(&transport).expect("spawn transport directly");
            let ready = process.read_available().await.expect("read ready");
            assert_eq!(String::from_utf8_lossy(&ready), "READY:direct-secret\n");
            process.terminate().await.expect("terminate child");
            let _ = process.wait().await.expect("wait after kill");

            cleanup_script(&script_path);
        });
    }

    #[test]
    fn lists_tools_calls_tool_and_reads_resources_over_jsonrpc() {
        let runtime = Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        runtime.block_on(async {
            let script_path = write_mcp_server_script();
            let transport = script_transport(&script_path);
            let mut process = McpStdioProcess::spawn(&transport).expect("spawn fake mcp server");

            let tools = process
                .list_tools(JsonRpcId::Number(2), None)
                .await
                .expect("list tools");
            assert_eq!(tools.error, None);
            assert_eq!(tools.id, JsonRpcId::Number(2));
            assert_eq!(
                tools.result,
                Some(McpListToolsResult {
                    tools: vec![McpTool {
                        name: "echo".to_string(),
                        description: Some("Echoes text".to_string()),
                        input_schema: Some(json!({
                            "type": "object",
                            "properties": {"text": {"type": "string"}},
                            "required": ["text"]
                        })),
                        annotations: None,
                        meta: None,
                    }],
                    next_cursor: None,
                })
            );

            let call = process
                .call_tool(
                    JsonRpcId::String("call-1".to_string()),
                    McpToolCallParams {
                        name: "echo".to_string(),
                        arguments: Some(json!({"text": "hello"})),
                        meta: None,
                    },
                )
                .await
                .expect("call tool");
            assert_eq!(call.error, None);
            let call_result = call.result.expect("tool result");
            assert_eq!(call_result.is_error, Some(false));
            assert_eq!(
                call_result.structured_content,
                Some(json!({"echoed": "hello"}))
            );
            assert_eq!(call_result.content.len(), 1);
            assert_eq!(call_result.content[0].kind, "text");
            assert_eq!(
                call_result.content[0].data.get("text"),
                Some(&json!("echo:hello"))
            );

            let resources = process
                .list_resources(JsonRpcId::Number(3), None)
                .await
                .expect("list resources");
            let resources_result = resources.result.expect("resources result");
            assert_eq!(resources_result.resources.len(), 1);
            assert_eq!(resources_result.resources[0].uri, "file://guide.txt");
            assert_eq!(
                resources_result.resources[0].mime_type.as_deref(),
                Some("text/plain")
            );

            let read = process
                .read_resource(
                    JsonRpcId::Number(4),
                    McpReadResourceParams {
                        uri: "file://guide.txt".to_string(),
                    },
                )
                .await
                .expect("read resource");
            assert_eq!(
                read.result,
                Some(McpReadResourceResult {
                    contents: vec![super::super::McpResourceContents {
                        uri: "file://guide.txt".to_string(),
                        mime_type: Some("text/plain".to_string()),
                        text: Some("contents for file://guide.txt".to_string()),
                        blob: None,
                        meta: None,
                    }],
                })
            );

            process.terminate().await.expect("terminate child");
            let _ = process.wait().await.expect("wait after kill");
            cleanup_script(&script_path);
        });
    }

    #[test]
    fn surfaces_jsonrpc_errors_from_tool_calls() {
        let runtime = Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        runtime.block_on(async {
            let script_path = write_mcp_server_script();
            let transport = script_transport(&script_path);
            let mut process = McpStdioProcess::spawn(&transport).expect("spawn fake mcp server");

            let response = process
                .call_tool(
                    JsonRpcId::Number(9),
                    McpToolCallParams {
                        name: "fail".to_string(),
                        arguments: None,
                        meta: None,
                    },
                )
                .await
                .expect("call tool with error response");

            assert_eq!(response.id, JsonRpcId::Number(9));
            assert!(response.result.is_none());
            assert_eq!(response.error.as_ref().map(|e| e.code), Some(-32001));
            assert_eq!(
                response.error.as_ref().map(|e| e.message.as_str()),
                Some("tool failed")
            );

            process.terminate().await.expect("terminate child");
            let _ = process.wait().await.expect("wait after kill");
            cleanup_script(&script_path);
        });
    }

    #[test]
    fn encode_frame_prefixes_content_length_header() {
        let framed = encode_frame(br#"{"jsonrpc":"2.0"}"#);
        let framed_text = String::from_utf8(framed).expect("utf8 frame");
        assert!(framed_text.starts_with("Content-Length: 17\r\n\r\n"));
        assert!(framed_text.ends_with("{\"jsonrpc\":\"2.0\"}"));
    }

    #[test]
    fn default_initialize_params_use_runtime_identity() {
        let params = default_initialize_params();
        assert_eq!(params.protocol_version, "2025-03-26");
        assert_eq!(params.client_info.name, "runtime");
        assert_eq!(params.client_info.version, env!("CARGO_PKG_VERSION"));
        assert_eq!(params.capabilities, json!({}));
    }

    #[test]
    fn test_process_error_handling() {
        let runtime = Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        runtime.block_on(async {
            let script_path = write_missing_header_script();
            let transport = script_transport(&script_path);
            let mut process = McpStdioProcess::spawn(&transport).expect("spawn malformed server");

            let error = process
                .read_frame()
                .await
                .expect_err("missing header should fail");
            assert_eq!(error.kind(), ErrorKind::InvalidData);
            assert_eq!(error.to_string(), "missing Content-Length header");

            let status = process.wait().await.expect("wait for exit");
            assert!(status.success());

            cleanup_script(&script_path);
        });
    }

    #[test]
    fn test_mcp_stdio_process_mock_spawn() {
        let transport = crate::mcp::client::McpStdioTransport {
            command: "/definitely/missing/acrawl-mcp".to_string(),
            args: Vec::new(),
            env: BTreeMap::new(),
        };

        let error = McpStdioProcess::spawn(&transport).expect_err("missing command should fail");
        assert_eq!(error.kind(), ErrorKind::NotFound);
    }
}

#[cfg(test)]
mod cross_platform_tests {
    use std::collections::BTreeMap;
    use std::io::ErrorKind;

    use crate::config::{
        ConfigSource, McpRemoteServerConfig, McpServerConfig, ScopedMcpServerConfig,
    };
    use crate::mcp::McpClientBootstrap;

    use super::{default_initialize_params, encode_frame, spawn_mcp_stdio_process};

    #[test]
    fn encode_frame_formats_content_length_and_preserves_payload() {
        let payload = b"{\"jsonrpc\":\"2.0\",\"method\":\"test\"}";
        let framed = encode_frame(payload);
        let text = String::from_utf8(framed).expect("valid utf8");
        assert!(text.starts_with(&format!("Content-Length: {}\r\n\r\n", payload.len())));
        assert!(text.ends_with("{\"jsonrpc\":\"2.0\",\"method\":\"test\"}"));
    }

    #[test]
    fn encode_frame_handles_empty_payload() {
        let framed = encode_frame(b"");
        let text = String::from_utf8(framed).expect("valid utf8");
        assert_eq!(text, "Content-Length: 0\r\n\r\n");
    }

    #[test]
    fn spawn_rejects_non_stdio_transport_with_descriptive_error() {
        let config = ScopedMcpServerConfig {
            scope: ConfigSource::Local,
            config: McpServerConfig::Http(McpRemoteServerConfig {
                url: "https://example.test/mcp".to_string(),
                headers: BTreeMap::new(),
                headers_helper: None,
                oauth: None,
            }),
        };
        let bootstrap = McpClientBootstrap::from_scoped_config("http-server", &config);
        let error = spawn_mcp_stdio_process(&bootstrap).expect_err("non-stdio should fail");
        assert_eq!(error.kind(), ErrorKind::InvalidInput);
        assert!(error.to_string().contains("http-server"));
        assert!(error.to_string().contains("not stdio"));
    }

    #[test]
    fn default_initialize_params_report_runtime_name_and_version() {
        let params = default_initialize_params();
        assert_eq!(params.client_info.name, "runtime");
        assert!(!params.client_info.version.is_empty());
        assert_eq!(params.protocol_version, "2025-03-26");
    }
}
