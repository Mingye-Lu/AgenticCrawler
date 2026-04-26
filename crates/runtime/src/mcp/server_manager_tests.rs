use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::json;
use tokio::runtime::Builder;

use crate::config::{
    ConfigSource, McpRemoteServerConfig, McpSdkServerConfig, McpServerConfig, McpStdioServerConfig,
    McpWebSocketServerConfig, ScopedMcpServerConfig,
};

use super::{mcp_tool_name, McpServerManager, McpServerManagerError};

fn temp_dir() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("runtime-mcp-manager-{nanos}"))
}

const MANAGER_MCP_SERVER_PY: &str = "\
#!/usr/bin/env python3
import json, os, sys

LABEL = os.environ.get('MCP_SERVER_LABEL', 'server')
LOG_PATH = os.environ.get('MCP_LOG_PATH')
initialize_count = 0

def log(method):
    if LOG_PATH:
        with open(LOG_PATH, 'a', encoding='utf-8') as handle:
            handle.write(f'{method}\\n')

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
    log(method)
    if 'id' not in request:
        continue
    if method == 'initialize':
        initialize_count += 1
        send_message({
            'jsonrpc': '2.0',
            'id': request['id'],
            'result': {
                'protocolVersion': request['params']['protocolVersion'],
                'capabilities': {'tools': {}},
                'serverInfo': {'name': LABEL, 'version': '1.0.0'}
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
                        'description': f'Echo tool for {LABEL}',
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
        text = args.get('text', '')
        send_message({
            'jsonrpc': '2.0',
            'id': request['id'],
            'result': {
                'content': [{'type': 'text', 'text': f'{LABEL}:{text}'}],
                'structuredContent': {
                    'server': LABEL,
                    'echoed': text,
                    'initializeCount': initialize_count
                },
                'isError': False
            }
        })
    else:
        send_message({
            'jsonrpc': '2.0',
            'id': request['id'],
            'error': {'code': -32601, 'message': f'unknown method: {method}'},
        })
";

fn write_manager_mcp_server_script() -> PathBuf {
    let root = temp_dir();
    fs::create_dir_all(&root).expect("temp dir");
    let script_path = root.join("manager-mcp-server.py");
    fs::write(&script_path, MANAGER_MCP_SERVER_PY).expect("write script");
    let mut permissions = fs::metadata(&script_path).expect("metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&script_path, permissions).expect("chmod");
    script_path
}

fn cleanup_script(script_path: &Path) {
    fs::remove_file(script_path).expect("cleanup script");
    fs::remove_dir_all(script_path.parent().expect("script parent")).expect("cleanup dir");
}

fn manager_server_config(
    script_path: &Path,
    label: &str,
    log_path: &Path,
) -> ScopedMcpServerConfig {
    ScopedMcpServerConfig {
        scope: ConfigSource::Local,
        config: McpServerConfig::Stdio(McpStdioServerConfig {
            command: "python3".to_string(),
            args: vec![script_path.to_string_lossy().into_owned()],
            env: BTreeMap::from([
                ("MCP_SERVER_LABEL".to_string(), label.to_string()),
                (
                    "MCP_LOG_PATH".to_string(),
                    log_path.to_string_lossy().into_owned(),
                ),
            ]),
        }),
    }
}

#[test]
fn manager_discovers_tools_from_stdio_config() {
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let script_path = write_manager_mcp_server_script();
        let root = script_path.parent().expect("script parent");
        let log_path = root.join("alpha.log");
        let servers = BTreeMap::from([(
            "alpha".to_string(),
            manager_server_config(&script_path, "alpha", &log_path),
        )]);
        let mut manager = McpServerManager::from_servers(&servers);

        let tools = manager.discover_tools().await.expect("discover tools");

        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].server_name, "alpha");
        assert_eq!(tools[0].raw_name, "echo");
        assert_eq!(tools[0].qualified_name, mcp_tool_name("alpha", "echo"));
        assert_eq!(tools[0].tool.name, "echo");
        assert!(manager.unsupported_servers().is_empty());

        manager.shutdown().await.expect("shutdown");
        cleanup_script(&script_path);
    });
}

#[test]
fn test_server_manager_tool_routing() {
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let script_path = write_manager_mcp_server_script();
        let root = script_path.parent().expect("script parent");
        let alpha_log = root.join("alpha.log");
        let beta_log = root.join("beta.log");
        let servers = BTreeMap::from([
            (
                "alpha".to_string(),
                manager_server_config(&script_path, "alpha", &alpha_log),
            ),
            (
                "beta".to_string(),
                manager_server_config(&script_path, "beta", &beta_log),
            ),
        ]);
        let mut manager = McpServerManager::from_servers(&servers);

        let tools = manager.discover_tools().await.expect("discover tools");
        assert_eq!(tools.len(), 2);

        let alpha = manager
            .call_tool(
                &mcp_tool_name("alpha", "echo"),
                Some(json!({"text": "hello"})),
            )
            .await
            .expect("call alpha tool");
        let beta = manager
            .call_tool(
                &mcp_tool_name("beta", "echo"),
                Some(json!({"text": "world"})),
            )
            .await
            .expect("call beta tool");

        assert_eq!(
            alpha
                .result
                .as_ref()
                .and_then(|result| result.structured_content.as_ref())
                .and_then(|value| value.get("server")),
            Some(&json!("alpha"))
        );
        assert_eq!(
            beta.result
                .as_ref()
                .and_then(|result| result.structured_content.as_ref())
                .and_then(|value| value.get("server")),
            Some(&json!("beta"))
        );

        manager.shutdown().await.expect("shutdown");
        cleanup_script(&script_path);
    });
}

#[test]
fn manager_records_unsupported_non_stdio_servers_without_panicking() {
    let servers = BTreeMap::from([
        (
            "http".to_string(),
            ScopedMcpServerConfig {
                scope: ConfigSource::Local,
                config: McpServerConfig::Http(McpRemoteServerConfig {
                    url: "https://example.test/mcp".to_string(),
                    headers: BTreeMap::new(),
                    headers_helper: None,
                    oauth: None,
                }),
            },
        ),
        (
            "sdk".to_string(),
            ScopedMcpServerConfig {
                scope: ConfigSource::Local,
                config: McpServerConfig::Sdk(McpSdkServerConfig {
                    name: "sdk-server".to_string(),
                }),
            },
        ),
        (
            "ws".to_string(),
            ScopedMcpServerConfig {
                scope: ConfigSource::Local,
                config: McpServerConfig::Ws(McpWebSocketServerConfig {
                    url: "wss://example.test/mcp".to_string(),
                    headers: BTreeMap::new(),
                    headers_helper: None,
                }),
            },
        ),
    ]);

    let manager = McpServerManager::from_servers(&servers);
    let unsupported = manager.unsupported_servers();

    assert_eq!(unsupported.len(), 3);
    assert_eq!(unsupported[0].server_name, "http");
    assert_eq!(unsupported[1].server_name, "sdk");
    assert_eq!(unsupported[2].server_name, "ws");
}

#[test]
fn manager_shutdown_terminates_spawned_children_and_is_idempotent() {
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let script_path = write_manager_mcp_server_script();
        let root = script_path.parent().expect("script parent");
        let log_path = root.join("alpha.log");
        let servers = BTreeMap::from([(
            "alpha".to_string(),
            manager_server_config(&script_path, "alpha", &log_path),
        )]);
        let mut manager = McpServerManager::from_servers(&servers);

        manager.discover_tools().await.expect("discover tools");
        manager.shutdown().await.expect("first shutdown");
        manager.shutdown().await.expect("second shutdown");

        cleanup_script(&script_path);
    });
}

#[test]
fn manager_reuses_spawned_server_between_discovery_and_call() {
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let script_path = write_manager_mcp_server_script();
        let root = script_path.parent().expect("script parent");
        let log_path = root.join("alpha.log");
        let servers = BTreeMap::from([(
            "alpha".to_string(),
            manager_server_config(&script_path, "alpha", &log_path),
        )]);
        let mut manager = McpServerManager::from_servers(&servers);

        manager.discover_tools().await.expect("discover tools");
        let response = manager
            .call_tool(
                &mcp_tool_name("alpha", "echo"),
                Some(json!({"text": "reuse"})),
            )
            .await
            .expect("call tool");

        assert_eq!(
            response
                .result
                .as_ref()
                .and_then(|result| result.structured_content.as_ref())
                .and_then(|value| value.get("initializeCount")),
            Some(&json!(1))
        );

        let log = fs::read_to_string(&log_path).expect("read log");
        assert_eq!(log.lines().filter(|line| *line == "initialize").count(), 1);
        assert_eq!(
            log.lines().collect::<Vec<_>>(),
            vec![
                "initialize",
                "notifications/initialized",
                "tools/list",
                "tools/call"
            ]
        );

        manager.shutdown().await.expect("shutdown");
        cleanup_script(&script_path);
    });
}

#[test]
fn manager_reports_unknown_qualified_tool_name() {
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let script_path = write_manager_mcp_server_script();
        let root = script_path.parent().expect("script parent");
        let log_path = root.join("alpha.log");
        let servers = BTreeMap::from([(
            "alpha".to_string(),
            manager_server_config(&script_path, "alpha", &log_path),
        )]);
        let mut manager = McpServerManager::from_servers(&servers);

        let error = manager
            .call_tool(
                &mcp_tool_name("alpha", "missing"),
                Some(json!({"text": "nope"})),
            )
            .await
            .expect_err("unknown qualified tool should fail");

        match error {
            McpServerManagerError::UnknownTool { qualified_name } => {
                assert_eq!(qualified_name, mcp_tool_name("alpha", "missing"));
            }
            other => panic!("expected unknown tool error, got {other:?}"),
        }

        cleanup_script(&script_path);
    });
}
