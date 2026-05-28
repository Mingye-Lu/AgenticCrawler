use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

pub(super) async fn serve_health(mut stream: TcpStream, port: u16) {
    let mut drain = vec![0u8; 4096];
    let _ = stream.read(&mut drain).await;

    let body = serde_json::json!({
        "service": "acrawl",
        "version": env!("CARGO_PKG_VERSION"),
        "port": port,
    })
    .to_string();

    let resp = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    let _ = stream.write_all(resp.as_bytes()).await;
}

pub(super) async fn send_raw_http_error(mut stream: TcpStream, status: u16, message: &str) {
    let mut drain = vec![0u8; 4096];
    let _ = stream.read(&mut drain).await;

    let resp = format!(
        "HTTP/1.1 {status} Error\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        message.len(),
        message
    );
    let _ = stream.write_all(resp.as_bytes()).await;
}
