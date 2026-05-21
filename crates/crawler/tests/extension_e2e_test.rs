use std::sync::Arc;

use crawler::{BrowserBackend, CrawlerAgent, ExtensionBridge, SharedBridge, WsBridgeServer};
use futures_util::{SinkExt, StreamExt};
use runtime::ToolExecutor;
use serde_json::{json, Value};
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::Message;

async fn start_server_and_connect() -> (
    WsBridgeServer,
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
) {
    let token = "test-token-abc".to_string();
    let server = WsBridgeServer::start(0, token.clone())
        .await
        .expect("server should start on ephemeral port");
    let port = server.port();

    let url = format!("ws://127.0.0.1:{port}/bridge?token={token}");
    let (ws, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("should connect to bridge server");

    (server, ws)
}

async fn respond_to_command(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    expected_action: &str,
    result: Option<Value>,
) {
    let msg = ws
        .next()
        .await
        .expect("should receive message")
        .expect("msg ok");
    let text = msg.into_text().expect("should be text");
    let cmd: Value = serde_json::from_str(&text).expect("should parse command");
    assert_eq!(
        cmd["action"].as_str().unwrap(),
        expected_action,
        "expected action {expected_action}, got: {text}"
    );
    let id = cmd["id"].as_u64().unwrap();
    let response = json!({ "id": id, "ok": true, "result": result });
    ws.send(Message::Text(
        serde_json::to_string(&response).unwrap().into(),
    ))
    .await
    .expect("should send response");
}

#[tokio::test]
async fn extension_bridge_e2e_tool_routes_through_websocket() {
    let (mut server, mut ws) = start_server_and_connect().await;

    assert!(
        server
            .wait_for_connection(std::time::Duration::from_secs(5))
            .await,
        "extension should connect within timeout"
    );

    // Create ExtensionBridge from the server's command sender
    let sender = server.command_sender();
    let connected = server.connection_watcher();
    let bridge = ExtensionBridge::new(sender, connected);
    let shared: SharedBridge = Arc::new(Mutex::new(
        Box::new(bridge) as Box<dyn BrowserBackend + Send>
    ));

    // Set up a CrawlerAgent with extension bridge
    let registry = crawler::ToolRegistry::new_with_core_tools();
    let mut agent = CrawlerAgent::new_lazy(registry);
    agent.set_shared_bridge(shared);

    // Execute click tool — should route through the WebSocket to our fake extension
    let handle =
        tokio::spawn(async move { agent.execute("click", r##"{"selector": "#btn"}"##).await });

    // Respond to commands as the fake extension
    respond_to_command(&mut ws, "switch_tab", Some(json!({"url": "", "title": ""}))).await;
    respond_to_command(&mut ws, "click", None).await;
    // post_action_page_state: switch_tab + page_map
    respond_to_command(&mut ws, "switch_tab", Some(json!({"url": "", "title": ""}))).await;
    respond_to_command(
        &mut ws,
        "page_map",
        Some(json!({
            "headings": [],
            "landmarks": [],
            "forms": [],
            "links": [],
            "interactive": {},
            "meta": {"url": "https://test.com", "title": "Test", "description": ""}
        })),
    )
    .await;

    let result = handle.await.expect("task completes");
    let output = result.expect("click succeeds through extension bridge");
    assert!(
        output.contains("Clicked element: #btn"),
        "unexpected: {output}"
    );
}

#[tokio::test]
async fn extension_mode_flag_blocks_cloakbrowser_fallback() {
    let mut agent = CrawlerAgent::new_lazy(crawler::ToolRegistry::new());
    agent.set_extension_mode(true);

    let err = agent
        .ensure_browser()
        .await
        .expect_err("should reject immediately");
    assert!(err.to_string().contains("Extension mode active"));
}
