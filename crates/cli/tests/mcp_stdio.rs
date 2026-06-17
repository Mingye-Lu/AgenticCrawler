use std::fs;
use std::io::{BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use runtime::{encode_mcp_frame, read_mcp_frame};
use serde_json::{json, Value};

struct TestServer {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    stderr: ChildStderr,
    config_home: PathBuf,
}

impl TestServer {
    fn spawn() -> Self {
        let config_home = unique_temp_dir("acrawl-mcp-stdio");
        fs::create_dir_all(&config_home).expect("create temp config home");
        fs::write(
            config_home.join("settings.json"),
            r#"{"headless":true,"output_dir":"output"}"#,
        )
        .expect("write settings.json");

        let mut child = Command::new(env!("CARGO_BIN_EXE_acrawl"))
            .arg("mcp")
            .env("ACRAWL_CONFIG_HOME", &config_home)
            .env_remove("HEADLESS")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn acrawl mcp");

        let stdin = child.stdin.take().expect("child stdin");
        let stdout = BufReader::new(child.stdout.take().expect("child stdout"));
        let stderr = child.stderr.take().expect("child stderr");

        Self {
            child,
            stdin: Some(stdin),
            stdout,
            stderr,
            config_home,
        }
    }

    fn send(&mut self, payload: &Value) {
        let body = serde_json::to_vec(payload).expect("serialize request");
        let framed = encode_mcp_frame(&body);
        let stdin = self.stdin.as_mut().expect("child stdin available");
        stdin.write_all(&framed).expect("write framed request");
        stdin.flush().expect("flush framed request");
    }

    fn read_response(&mut self) -> Value {
        let payload = read_mcp_frame(&mut self.stdout).expect("read framed response");
        serde_json::from_slice(&payload).expect("parse json-rpc response")
    }

    fn read_stderr(&mut self) -> String {
        use std::io::Read;

        let mut stderr = String::new();
        let _ = self.stderr.read_to_string(&mut stderr);
        stderr
    }

    fn shutdown(mut self) {
        let _ = self.stdin.take();
        let status = self.child.wait().expect("wait for server exit");
        assert!(status.success(), "server stderr: {}", self.read_stderr());
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        if let Some(stdin) = self.stdin.as_mut() {
            let _ = stdin.flush();
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_dir_all(&self.config_home);
    }
}

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    std::env::temp_dir().join(format!("{prefix}-{nanos}"))
}

fn tool_names(response: &Value) -> Vec<&str> {
    response["result"]["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .map(|tool| tool["name"].as_str().expect("tool name"))
        .collect()
}

fn assert_jsonrpc_error(response: &Value, id: i64, code: i64, message_fragment: &str) {
    assert_eq!(response["jsonrpc"], "2.0");
    assert_eq!(response["id"], id);
    assert_eq!(response["error"]["code"], code);
    assert!(response["error"]["message"]
        .as_str()
        .expect("error message")
        .contains(message_fragment));
}

fn read_json_line(reader: &mut BufReader<ChildStdout>) -> Value {
    use std::io::BufRead;

    let mut line = String::new();
    reader
        .read_line(&mut line)
        .expect("read json line response");
    serde_json::from_str(line.trim_end_matches(['\r', '\n'])).expect("parse json line response")
}

#[test]
fn stdio_server_handles_initialize_list_and_tool_call() {
    let mut server = TestServer::spawn();

    server.send(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {}
    }));
    let initialize = server.read_response();
    assert_eq!(initialize["jsonrpc"], "2.0");
    assert_eq!(initialize["id"], 1);
    assert_eq!(
        initialize["result"]["serverInfo"]["name"],
        "acrawl-mcp-server"
    );
    assert_eq!(initialize["result"]["capabilities"], json!({ "tools": {} }));

    server.send(&json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized"
    }));

    server.send(&json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/list"
    }));
    let list = server.read_response();
    let names = tool_names(&list);
    assert!(names.contains(&"navigate"));
    assert!(names.contains(&"click"));
    assert!(names.contains(&"screenshot"));
    assert!(names.contains(&"run_goal"));
    assert!(!names.contains(&"fork"));
    assert!(!names.contains(&"list_builtin_tools"));
    assert_eq!(names.len(), 35);

    server.send(&json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": {
            "name": "list_builtin_tools",
            "arguments": {}
        }
    }));
    let tool_call = server.read_response();
    assert_eq!(tool_call["jsonrpc"], "2.0");
    assert_eq!(tool_call["id"], 3);
    assert_eq!(tool_call["error"]["code"], -32601);

    server.shutdown();
}

#[test]
fn stdio_server_returns_jsonrpc_error_for_unknown_method() {
    let mut server = TestServer::spawn();

    server.send(&json!({
        "jsonrpc": "2.0",
        "id": 10,
        "method": "bogus/method"
    }));
    let response = server.read_response();
    assert_jsonrpc_error(&response, 10, -32601, "method not found");

    server.shutdown();
}

#[test]
fn stdio_server_returns_jsonrpc_error_for_unknown_tool() {
    let mut server = TestServer::spawn();

    server.send(&json!({
        "jsonrpc": "2.0",
        "id": 11,
        "method": "tools/call",
        "params": {
            "name": "bogus_tool",
            "arguments": {}
        }
    }));
    let response = server.read_response();
    assert_jsonrpc_error(&response, 11, -32601, "unknown tool");

    server.shutdown();
}

#[test]
fn stdio_server_returns_jsonrpc_error_when_run_goal_is_missing_goal() {
    let mut server = TestServer::spawn();

    server.send(&json!({
        "jsonrpc": "2.0",
        "id": 12,
        "method": "tools/call",
        "params": {
            "name": "run_goal",
            "arguments": {}
        }
    }));
    let response = server.read_response();
    assert_jsonrpc_error(&response, 12, -32602, "missing required parameter: goal");

    server.shutdown();
}

#[test]
fn stdio_server_run_script_returns_script_id_and_survives() {
    let mut server = TestServer::spawn();

    server.send(&json!({
        "jsonrpc": "2.0", "id": 1,
        "method": "initialize",
        "params": { "protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": { "name": "e2e", "version": "1" } }
    }));
    server.read_response();

    server.send(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized", "params": {} }));

    server.send(&json!({
        "jsonrpc": "2.0", "id": 2,
        "method": "tools/call",
        "params": {
            "name": "run_script",
            "arguments": {
                "script": {
                    "schema_version": 1,
                    "steps": [
                        { "type": "assign", "variable": "n", "value": { "kind": "literal", "value": 7 } },
                        { "type": "collect", "value": { "kind": "variable", "value": "n" } },
                        { "type": "yield",   "value": { "kind": "literal",  "value": "ok" } }
                    ]
                },
                "limits": {
                    "max_steps": 10, "max_timeout_secs": 30,
                    "max_output_bytes": 1_048_576, "max_parallel_branches": 2,
                    "max_script_size_bytes": 65_536, "max_nesting_depth": 5,
                    "per_step_timeout_secs": 10
                }
            }
        }
    }));
    let run_response = server.read_response();
    assert_eq!(run_response["jsonrpc"], "2.0");
    assert_eq!(run_response["id"], 2);

    let content_text = run_response["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or("");

    if run_response["result"]["isError"] == true
        && content_text.contains("failed to launch browser")
    {
        return;
    }

    assert_eq!(
        run_response["result"]["isError"], false,
        "run_script failed: {content_text}"
    );
    let spawned: Value = serde_json::from_str(content_text).expect("parse script_id response");
    let script_id = spawned["script_id"].as_str().expect("script_id present");
    assert!(
        script_id.starts_with("scr_"),
        "script_id should have scr_ prefix: {script_id}"
    );

    server.send(&json!({
        "jsonrpc": "2.0", "id": 3,
        "method": "tools/call",
        "params": { "name": "wait_for_scripts", "arguments": {} }
    }));
    let wait_response = server.read_response();
    assert_eq!(wait_response["result"]["isError"], false);

    let results: Vec<Value> = serde_json::from_str(
        wait_response["result"]["content"][0]["text"]
            .as_str()
            .expect("result text"),
    )
    .expect("parse results");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["script_id"], script_id);
    assert_eq!(results[0]["status"], "Completed");
    assert_eq!(results[0]["extracted_data"][0], 7);
    assert_eq!(results[0]["yielded_data"][0], "ok");

    server.send(&json!({
        "jsonrpc": "2.0", "id": 4,
        "method": "tools/call",
        "params": { "name": "list_scripts", "arguments": {} }
    }));
    let alive = server.read_response();
    assert_eq!(
        alive["id"], 4,
        "server must still be alive after run_script"
    );

    server.shutdown();
}

#[test]
fn stdio_server_supports_line_delimited_jsonrpc() {
    let mut server = TestServer::spawn();

    let request = json!({
        "jsonrpc": "2.0",
        "id": 21,
        "method": "tools/list"
    });
    let stdin = server.stdin.as_mut().expect("child stdin available");
    stdin
        .write_all(
            format!(
                "{}\n",
                serde_json::to_string(&request).expect("serialize request")
            )
            .as_bytes(),
        )
        .expect("write json line request");
    stdin.flush().expect("flush json line request");

    let response = read_json_line(&mut server.stdout);
    let names = tool_names(&response);
    assert_eq!(response["jsonrpc"], "2.0");
    assert_eq!(response["id"], 21);
    assert!(names.contains(&"navigate"));
    assert!(names.contains(&"run_goal"));
    assert_eq!(names.len(), 35);

    server.shutdown();
}
