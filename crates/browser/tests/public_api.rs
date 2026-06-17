use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::Mutex;

use browser::{
    BridgeError, BrowserBackend, BrowserContext, BrowserState, FetchError, FetchRouter,
    FetchedPage, PageInfo, SharedBridge, WsBridgeError,
};

#[derive(Debug)]
struct MockBrowserBackend {
    current_tab: i64,
    navigate_count: usize,
}

impl MockBrowserBackend {
    fn new() -> Self {
        Self {
            current_tab: 0,
            navigate_count: 0,
        }
    }
}

#[async_trait]
impl BrowserBackend for MockBrowserBackend {
    async fn navigate(&mut self, url: &str) -> Result<PageInfo, BridgeError> {
        self.navigate_count += 1;
        Ok(PageInfo {
            title: format!("Title for {url}"),
            html: format!("<html><body>Mock page for {url}</body></html>"),
        })
    }

    async fn new_page(&mut self, _url: Option<&str>) -> Result<usize, BridgeError> {
        Ok(1)
    }

    async fn close_page(&mut self, _page_index: usize) -> Result<(), BridgeError> {
        Ok(())
    }

    async fn scroll(&mut self, _direction: &str, _pixels: i64) -> Result<(), BridgeError> {
        Ok(())
    }

    async fn page_map(
        &mut self,
        _scope: Option<&str>,
        _compound_enrichment: bool,
    ) -> Result<serde_json::Value, BridgeError> {
        Ok(serde_json::json!({"headings": []}))
    }

    async fn read_content(
        &mut self,
        _heading: Option<&str>,
        _selector: Option<&str>,
        _offset: usize,
        _max_chars: usize,
    ) -> Result<serde_json::Value, BridgeError> {
        Ok(serde_json::json!({"text": "mock content"}))
    }

    async fn wait_for_selector(
        &mut self,
        _selector: &str,
        _timeout_ms: u64,
        _state: Option<&str>,
    ) -> Result<bool, BridgeError> {
        Ok(true)
    }

    async fn select_option(&mut self, _selector: &str, _value: &str) -> Result<(), BridgeError> {
        Ok(())
    }

    async fn evaluate(&mut self, _script: &str) -> Result<serde_json::Value, BridgeError> {
        Ok(serde_json::json!(null))
    }

    async fn hover(&mut self, _selector: &str) -> Result<(), BridgeError> {
        Ok(())
    }

    async fn press_key(&mut self, _key: &str, _selector: Option<&str>) -> Result<(), BridgeError> {
        Ok(())
    }

    async fn switch_tab(&mut self, index: i64) -> Result<serde_json::Value, BridgeError> {
        self.current_tab = index;
        Ok(serde_json::json!({"tab": index}))
    }

    async fn export_cookies(&mut self) -> Result<BrowserState, BridgeError> {
        Ok(BrowserState {
            cookies: serde_json::json!([]),
            local_storage: serde_json::json!({}),
            url: "about:blank".to_string(),
        })
    }

    async fn import_cookies(&mut self, _state: &BrowserState) -> Result<(), BridgeError> {
        Ok(())
    }

    async fn import_cookies_only(&mut self, _state: &BrowserState) -> Result<(), BridgeError> {
        Ok(())
    }

    async fn import_local_storage(&mut self, _state: &BrowserState) -> Result<(), BridgeError> {
        Ok(())
    }

    async fn list_resources(&mut self) -> Result<serde_json::Value, BridgeError> {
        Ok(serde_json::json!({"links": [], "images": []}))
    }

    async fn save_file(&mut self, _url: &str, _path: &str) -> Result<String, BridgeError> {
        Ok("saved.txt".to_string())
    }

    async fn click(&mut self, _selector: &str) -> Result<(), BridgeError> {
        Ok(())
    }

    async fn click_at(&mut self, _x: f64, _y: f64) -> Result<(), BridgeError> {
        Ok(())
    }

    async fn fill(&mut self, _selector: &str, _value: &str) -> Result<(), BridgeError> {
        Ok(())
    }

    async fn screenshot(
        &mut self,
        _options: &browser::ScreenshotOptions<'_>,
    ) -> Result<(String, usize), BridgeError> {
        Ok(("base64data".to_string(), 100))
    }

    async fn go_back(&mut self) -> Result<String, BridgeError> {
        Ok("about:blank".to_string())
    }

    async fn set_device(
        &mut self,
        _options: &serde_json::Value,
    ) -> Result<serde_json::Value, BridgeError> {
        Ok(serde_json::json!({"success": true}))
    }
}

fn make_shared_bridge() -> SharedBridge {
    let backend: Box<dyn BrowserBackend + Send> = Box::new(MockBrowserBackend::new());
    Arc::new(Mutex::new(backend))
}

#[test]
fn browser_backend_trait_is_object_safe() {
    let backend = MockBrowserBackend::new();
    let _boxed: Box<dyn BrowserBackend + Send> = Box::new(backend);
}

#[test]
fn shared_bridge_type_alias_works() {
    let _bridge: SharedBridge = make_shared_bridge();
}

#[test]
fn browser_context_new_sets_page_index_zero() {
    let ctx = BrowserContext::new(make_shared_bridge());
    assert_eq!(ctx.page_index(), 0);
}

#[test]
fn browser_context_new_shared_respects_page_index() {
    let ctx = BrowserContext::new_shared(make_shared_bridge(), 3);
    assert_eq!(ctx.page_index(), 3);
}

#[test]
fn browser_context_set_page_index() {
    let mut ctx = BrowserContext::new(make_shared_bridge());
    assert_eq!(ctx.page_index(), 0);
    ctx.set_page_index(5);
    assert_eq!(ctx.page_index(), 5);
}

#[tokio::test]
async fn browser_context_acquire_bridge_succeeds() {
    let mut ctx = BrowserContext::new(make_shared_bridge());
    let guard = ctx.acquire_bridge().await;
    assert!(guard.is_ok());
}

#[tokio::test]
async fn browser_context_navigate_through_bridge() {
    let mut ctx = BrowserContext::new(make_shared_bridge());
    let mut guard = ctx.acquire_bridge().await.unwrap();
    let page_info = guard.navigate("https://example.com").await.unwrap();
    assert_eq!(page_info.title, "Title for https://example.com");
    assert!(page_info.html.contains("example.com"));
}

#[test]
fn fetch_error_variants_display() {
    let browser_err = FetchError::Browser("timeout occurred".to_string());
    let display = format!("{browser_err}");
    assert!(display.contains("timeout occurred"));

    let status_err = FetchError::StatusError {
        status: 404,
        url: "https://example.com/missing".to_string(),
    };
    let display = format!("{status_err}");
    assert!(display.contains("404"));
    assert!(display.contains("example.com/missing"));

    let body_err = FetchError::BodyTooLarge {
        url: "https://big.com/file".to_string(),
        limit: 1024,
    };
    let display = format!("{body_err}");
    assert!(display.contains("1024"));
}

#[test]
fn fetch_router_construction_succeeds() {
    let router = FetchRouter::new();
    assert!(router.is_ok());
}

#[test]
fn fetched_page_struct_fields() {
    let page = FetchedPage {
        url: "https://example.com".to_string(),
        title: Some("Example".to_string()),
        html: "<html></html>".to_string(),
        text: "Example text".to_string(),
        markdown: "# Example".to_string(),
        fetched_via_browser: false,
    };
    assert_eq!(page.url, "https://example.com");
    assert_eq!(page.title.as_deref(), Some("Example"));
    assert!(!page.fetched_via_browser);
}

#[test]
fn bridge_error_variants_display() {
    let timeout_err = BridgeError::CommandTimeout {
        timeout: Duration::from_mins(1),
    };
    let display = format!("{timeout_err}");
    assert!(display.contains("60"));

    let protocol_err = BridgeError::Protocol("invalid frame".to_string());
    let display = format!("{protocol_err}");
    assert!(display.contains("invalid frame"));

    let child_closed = BridgeError::ChildClosed;
    let display = format!("{child_closed}");
    assert!(display.contains("closed unexpectedly"));

    let ext_disconnected = BridgeError::ExtensionDisconnected;
    let display = format!("{ext_disconnected}");
    assert!(display.contains("disconnected"));
}

#[test]
fn browser_state_serialization_roundtrip() {
    let state = BrowserState {
        cookies: serde_json::json!([{"name": "session", "value": "abc123"}]),
        local_storage: serde_json::json!({"theme": "dark"}),
        url: "https://example.com/dashboard".to_string(),
    };
    let json = serde_json::to_string(&state).unwrap();
    let deserialized: BrowserState = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.url, "https://example.com/dashboard");
    assert_eq!(deserialized.cookies[0]["name"], "session");
}

#[test]
fn generate_bridge_token_format() {
    let token = browser::generate_bridge_token();
    assert_eq!(token.len(), 64);
    assert!(token.chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn generate_bridge_token_uniqueness() {
    let t1 = browser::generate_bridge_token();
    let t2 = browser::generate_bridge_token();
    assert_ne!(t1, t2);
}

#[test]
fn ws_bridge_error_display() {
    let bind_err = WsBridgeError::Bind(std::io::Error::new(
        std::io::ErrorKind::AddrInUse,
        "port in use",
    ));
    let display = format!("{bind_err}");
    assert!(display.contains("bind"));
    assert!(display.contains("port in use"));

    let write_err = WsBridgeError::BridgeFileWrite(std::io::Error::new(
        std::io::ErrorKind::PermissionDenied,
        "access denied",
    ));
    let display = format!("{write_err}");
    assert!(display.contains("bridge.json"));
    assert!(display.contains("access denied"));
}

#[tokio::test]
async fn browser_context_set_navigated_url_tracking() {
    let mut ctx = BrowserContext::new(make_shared_bridge());
    ctx.set_navigated_url("https://example.com", true);
    let guard = ctx.acquire_bridge().await;
    assert!(guard.is_ok());
}

#[test]
fn browser_context_bridge_accessor() {
    let bridge = make_shared_bridge();
    let ctx = BrowserContext::new(bridge.clone());
    assert!(Arc::ptr_eq(ctx.bridge(), &bridge));
}
