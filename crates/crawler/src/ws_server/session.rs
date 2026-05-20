use std::collections::HashMap;
use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;

use super::BridgeResponse;

pub(super) type CommandRx = Arc<
    tokio::sync::Mutex<mpsc::Receiver<(super::BridgeCommand, oneshot::Sender<BridgeResponse>)>>,
>;

pub(super) async fn run_ws_session(
    ws: tokio_tungstenite::WebSocketStream<TcpStream>,
    command_rx: CommandRx,
) {
    let (mut sink, mut source) = ws.split();
    let mut rx = command_rx.lock().await;
    let mut pending: HashMap<u64, oneshot::Sender<BridgeResponse>> = HashMap::new();

    loop {
        tokio::select! {
            cmd_opt = rx.recv() => {
                let Some((cmd, resp_tx)) = cmd_opt else { break };
                pending.insert(cmd.id, resp_tx);
                let Ok(json) = serde_json::to_string(&cmd) else { continue };
                if sink.send(Message::Text(json.into())).await.is_err() {
                    break;
                }
            }
            msg_opt = source.next() => {
                let Some(Ok(msg)) = msg_opt else { break };
                match msg {
                    Message::Text(text) => {
                        handle_text_frame(&text, &mut pending, &mut sink).await;
                    }
                    Message::Close(_) => break,
                    _ => {}
                }
            }
        }
    }
}

async fn handle_text_frame(
    text: &str,
    pending: &mut HashMap<u64, oneshot::Sender<BridgeResponse>>,
    sink: &mut futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<TcpStream>,
        Message,
    >,
) {
    let Ok(val) = serde_json::from_str::<serde_json::Value>(text) else {
        return;
    };

    if val.get("type").and_then(|v| v.as_str()) == Some("ping") {
        let pong = serde_json::json!({"type": "pong"}).to_string();
        let _ = sink.send(Message::Text(pong.into())).await;
        return;
    }

    if let Ok(resp) = serde_json::from_value::<BridgeResponse>(val) {
        if let Some(tx) = pending.remove(&resp.id) {
            let _ = tx.send(resp);
        }
    }
}
