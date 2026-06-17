use axum::{http::HeaderMap, response::Html, routing::get, Router};
use std::net::SocketAddr;

pub struct TestServer {
    pub base_url: String,
    #[allow(dead_code)]
    pub ws_url: String,
    handle: tokio::task::JoinHandle<()>,
}

impl TestServer {
    pub async fn start() -> Self {
        let port = portpicker::pick_unused_port().expect("No free port");
        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        let base_url = format!("http://127.0.0.1:{port}");
        let ws_url = format!("ws://127.0.0.1:{port}/websocket-echo");

        let app = Router::new()
            .route("/network-basic", get(network_basic_handler))
            .route("/api/a", get(|| async { "a" }))
            .route("/api/b", get(|| async { "b" }))
            .route(
                "/api/404",
                get(|| async { axum::http::StatusCode::NOT_FOUND }),
            )
            .route("/console-basic", get(console_basic_handler))
            .route("/cookies-basic", get(cookies_basic_handler))
            .route("/storage-basic", get(storage_basic_handler))
            .route("/redirect-chain", get(redirect_chain_handler))
            .route("/redirect-target", get(redirect_target_handler))
            .route("/slow-response", get(slow_response_handler))
            .route("/health", get(|| async { "ok" }));

        let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        // Give it a moment to start
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        TestServer {
            base_url,
            ws_url,
            handle,
        }
    }

    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url)
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

// Fixture handlers — each serves predictable, testable content

async fn network_basic_handler() -> Html<String> {
    Html(
        r#"<!DOCTYPE html>
<html><body>
<script>
// Makes 3 known fetch calls: /api/a (200), /api/b (200), /api/404 (404)
Promise.all([
  fetch('/api/a'),
  fetch('/api/b'),
  fetch('/api/404')
]).catch(() => {});
</script>
</body></html>"#
            .to_string(),
    )
}

async fn console_basic_handler() -> Html<String> {
    Html(
        r#"<!DOCTYPE html>
<html><body>
<script>
console.log('hello from fixture');
console.warn('warning from fixture');
console.error('error from fixture');
// Also throw an error
setTimeout(() => { throw new Error('fixture exception'); }, 100);
</script>
</body></html>"#
            .to_string(),
    )
}

async fn cookies_basic_handler() -> (HeaderMap, Html<String>) {
    let mut headers = HeaderMap::new();
    headers.insert(
        "set-cookie",
        "secure_cookie=value1; Secure; HttpOnly; SameSite=Strict; Path=/"
            .parse()
            .unwrap(),
    );
    headers.append(
        "set-cookie",
        "insecure_cookie=value2; Path=/".parse().unwrap(),
    );
    (
        headers,
        Html("<html><body>cookies set</body></html>".to_string()),
    )
}

async fn storage_basic_handler() -> Html<String> {
    Html(
        r#"<!DOCTYPE html>
<html><body>
<script>
localStorage.setItem('fixture_key_1', 'fixture_value_1');
localStorage.setItem('fixture_key_2', 'fixture_value_2');
sessionStorage.setItem('session_key_1', 'session_value_1');
</script>
</body></html>"#
            .to_string(),
    )
}

async fn redirect_chain_handler() -> axum::response::Redirect {
    axum::response::Redirect::to("/redirect-target")
}

async fn redirect_target_handler() -> Html<String> {
    Html("<html><body>redirect destination</body></html>".to_string())
}

async fn slow_response_handler() -> Html<String> {
    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
    Html("<html><body>slow</body></html>".to_string())
}
