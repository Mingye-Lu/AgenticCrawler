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
            r#"{"headless":true,"workspace_dir":"workspace"}"#,
        )
        .expect("write settings.json");

        let mut child = Command::new(env!("CARGO_BIN_EXE_acrawl-mcp-server"))
            .env("ACRAWL_CONFIG_HOME", &config_home)
            .env_remove("HEADLESS")
            .env_remove("WORKSPACE_DIR")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn acrawl-mcp-server");

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

fn builtin_tool_names(response: &Value) -> Vec<&str> {
    response["result"]["structuredContent"]["tools"]
        .as_array()
        .expect("builtin tools array")
        .iter()
        .map(|tool| tool["name"].as_str().expect("builtin tool name"))
        .collect()
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
    assert_eq!(names, vec!["run_goal", "list_builtin_tools"]);

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
    assert_eq!(tool_call["result"]["isError"], false);
    assert_eq!(tool_call["result"]["structuredContent"]["tool_count"], 19);

    let builtin_names = builtin_tool_names(&tool_call);
    assert!(builtin_names.contains(&"navigate"));
    assert!(builtin_names.contains(&"wait_for_human"));

    server.shutdown();
}
