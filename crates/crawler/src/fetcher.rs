use std::fmt;
use std::time::Duration;

use reqwest::header::{HeaderMap, HeaderValue, USER_AGENT};
use reqwest::{Client, StatusCode};

use crate::browser::BrowserContext;
use crate::playwright::PageInfo;

#[derive(Debug, Clone)]
pub struct FetchedPage {
    pub url: String,
    pub title: Option<String>,
    pub html: String,
    pub text: String,
}

#[derive(Debug)]
pub enum FetchError {
    Http(reqwest::Error),
    Browser(String),
    StatusError { status: u16, url: String },
}

impl fmt::Display for FetchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Http(e) => write!(f, "HTTP fetch error: {e}"),
            Self::Browser(msg) => write!(f, "Browser fetch error: {msg}"),
            Self::StatusError { status, url } => {
                write!(f, "HTTP {status} for {url}")
            }
        }
    }
}

impl std::error::Error for FetchError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Http(e) => Some(e),
            Self::Browser(_) | Self::StatusError { .. } => None,
        }
    }
}

impl From<reqwest::Error> for FetchError {
    fn from(value: reqwest::Error) -> Self {
        Self::Http(value)
    }
}

const JS_FRAMEWORK_MARKERS: &[&str] = &[
    "__next_data__",
    "__nuxt",
    "__vue",
    "ng-app",
    "ng-version",
    "_react",
    "reactroot",
    "data-reactroot",
    "data-reactid",
];

const ESCALATION_STATUS_CODES: &[u16] = &[403, 429, 503];

const AUTH_REDIRECT_PATTERNS: &[&str] = &[
    "login",
    "signin",
    "sign-in",
    "auth",
    "sso",
    "accounts.google.com",
    "oauth",
];

const MIN_BODY_LENGTH: usize = 500;

fn needs_escalation(status: StatusCode, body: &str) -> bool {
    if ESCALATION_STATUS_CODES.contains(&status.as_u16()) {
        return true;
    }

    let lower = body.to_ascii_lowercase();

    if lower.trim().len() < MIN_BODY_LENGTH && lower.contains("<noscript") {
        return true;
    }

    for marker in JS_FRAMEWORK_MARKERS {
        if lower.contains(marker) {
            return true;
        }
    }

    for pattern in AUTH_REDIRECT_PATTERNS {
        if lower.contains(&format!("action=\"/{pattern}"))
            || lower.contains(&format!("url=/{pattern}"))
            || lower.contains(&format!("href=\"/{pattern}"))
            || lower.contains(&format!("action=\"{pattern}"))
        {
            return true;
        }
    }

    false
}

fn extract_title(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let start = lower.find("<title")?;
    let after_open = lower[start..].find('>')?;
    let content_start = start + after_open + 1;
    let end = lower[content_start..].find("</title>")?;
    let title = html[content_start..content_start + end].trim();
    if title.is_empty() {
        None
    } else {
        Some(title.to_string())
    }
}

fn extract_text(html: &str) -> String {
    let mut cleaned = remove_tag_blocks(html, "script");
    cleaned = remove_tag_blocks(&cleaned, "style");
    cleaned = remove_tag_blocks(&cleaned, "noscript");
    cleaned = remove_tag_blocks(&cleaned, "svg");

    let mut result = String::with_capacity(cleaned.len());
    let mut inside_tag = false;
    for ch in cleaned.chars() {
        if ch == '<' {
            inside_tag = true;
            result.push(' ');
            continue;
        }
        if ch == '>' {
            inside_tag = false;
            continue;
        }
        if !inside_tag {
            result.push(ch);
        }
    }

    let result = result
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ");

    collapse_whitespace(&result)
}

fn remove_tag_blocks(html: &str, tag: &str) -> String {
    let open_tag = format!("<{tag}");
    let close_tag = format!("</{tag}>");
    let mut result = String::with_capacity(html.len());
    let lower = html.to_ascii_lowercase();
    let mut cursor = 0;

    while cursor < html.len() {
        if let Some(start) = lower[cursor..].find(&open_tag) {
            let abs_start = cursor + start;
            result.push_str(&html[cursor..abs_start]);
            if let Some(end) = lower[abs_start..].find(&close_tag) {
                cursor = abs_start + end + close_tag.len();
            } else if let Some(gt) = html[abs_start..].find('>') {
                cursor = abs_start + gt + 1;
            } else {
                cursor = html.len();
            }
        } else {
            result.push_str(&html[cursor..]);
            break;
        }
    }

    result
}

fn collapse_whitespace(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut last_was_space = true;

    for ch in text.chars() {
        if ch.is_whitespace() {
            if !last_was_space {
                result.push(if ch == '\n' { '\n' } else { ' ' });
                last_was_space = true;
            }
        } else {
            result.push(ch);
            last_was_space = false;
        }
    }

    result.trim().to_string()
}

pub struct HttpFetcher {
    client: Client,
}

impl HttpFetcher {
    /// # Errors
    ///
    /// Returns an error if the reqwest client fails to build.
    pub fn new() -> Result<Self, FetchError> {
        Self::with_timeout(Duration::from_secs(30))
    }

    /// # Errors
    ///
    /// Returns an error if the reqwest client fails to build.
    pub fn with_timeout(timeout: Duration) -> Result<Self, FetchError> {
        let mut headers = HeaderMap::new();
        headers.insert(
            USER_AGENT,
            HeaderValue::from_static(
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) \
                 AppleWebKit/537.36 (KHTML, like Gecko) \
                 Chrome/131.0.0.0 Safari/537.36",
            ),
        );

        let client = Client::builder()
            .default_headers(headers)
            .timeout(timeout)
            .redirect(reqwest::redirect::Policy::limited(10))
            .build()?;

        Ok(Self { client })
    }

    /// # Errors
    ///
    /// Returns `FetchError::Http` on network or protocol errors.
    pub async fn fetch(&self, url: &str) -> Result<HttpResponse, FetchError> {
        let resp = self.client.get(url).send().await?;
        let status = resp.status();
        let final_url = resp.url().to_string();
        let body = resp.text().await?;

        Ok(HttpResponse {
            url: final_url,
            status,
            body,
        })
    }
}

#[derive(Debug)]
pub struct HttpResponse {
    pub url: String,
    pub status: StatusCode,
    pub body: String,
}

/// Returns `true` when the `HEADLESS` env var is unset or is not one of the
/// explicit "off" values (`false`, `0`, `no`, `off`).  This mirrors the
/// `parseHeadless()` logic in the embedded Node.js Playwright bridge script.
fn is_headless() -> bool {
    match std::env::var("HEADLESS") {
        Err(_) => true,
        Ok(val) => {
            let v = val.trim().to_lowercase();
            !matches!(v.as_str(), "false" | "0" | "no" | "off")
        }
    }
}

pub struct FetchRouter {
    http: HttpFetcher,
    /// When `true` (headed mode), navigate always goes through the browser so
    /// the user can see the page loading in the Chromium window.  When `false`
    /// (headless, the default), the faster HTTP-first path is used.
    prefer_browser: bool,
}

impl FetchRouter {
    /// # Errors
    ///
    /// Returns an error if the HTTP client cannot be constructed.
    pub fn new() -> Result<Self, FetchError> {
        Ok(Self {
            http: HttpFetcher::new()?,
            prefer_browser: !is_headless(),
        })
    }

    /// Fetch a page, trying HTTP first and escalating to Playwright when
    /// JS-rendering signals are detected.
    ///
    /// # Errors
    ///
    /// Returns a `FetchError` if both HTTP and browser fetching fail.
    pub async fn fetch(
        &self,
        url: &str,
        browser: Option<&mut BrowserContext>,
    ) -> Result<FetchedPage, FetchError> {
        if self.prefer_browser {
            if let Some(ctx) = browser {
                return Self::fetch_via_browser(ctx, url).await;
            }
        }

        let http_result = self.http.fetch(url).await;

        match http_result {
            Ok(resp) => {
                if needs_escalation(resp.status, &resp.body) {
                    if let Some(ctx) = browser {
                        return Self::fetch_via_browser(ctx, url).await;
                    }
                }
                if (resp.status.is_server_error() || resp.status.is_client_error())
                    && browser.is_none()
                {
                    return Err(FetchError::StatusError {
                        status: resp.status.as_u16(),
                        url: url.to_string(),
                    });
                }
                Ok(http_response_to_page(resp))
            }
            Err(e) => {
                if let Some(ctx) = browser {
                    Self::fetch_via_browser(ctx, url).await
                } else {
                    Err(e)
                }
            }
        }
    }

    async fn fetch_via_browser(
        ctx: &mut BrowserContext,
        url: &str,
    ) -> Result<FetchedPage, FetchError> {
        let page_info: PageInfo = ctx
            .acquire_bridge()
            .await
            .navigate(url)
            .await
            .map_err(|e| FetchError::Browser(e.to_string()))?;

        let text = extract_text(&page_info.html);
        let title = if page_info.title.is_empty() {
            extract_title(&page_info.html)
        } else {
            Some(page_info.title)
        };

        Ok(FetchedPage {
            url: url.to_string(),
            title,
            html: page_info.html,
            text,
        })
    }
}

fn http_response_to_page(resp: HttpResponse) -> FetchedPage {
    let title = extract_title(&resp.body);
    let text = extract_text(&resp.body);
    FetchedPage {
        url: resp.url,
        title,
        html: resp.body,
        text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_title_from_basic_html() {
        let html = "<html><head><title>Hello World</title></head></html>";
        assert_eq!(extract_title(html), Some("Hello World".to_string()));
    }

    #[test]
    fn extract_title_with_attributes() {
        let html = r#"<html><title lang="en">My Page</title></html>"#;
        assert_eq!(extract_title(html), Some("My Page".to_string()));
    }

    #[test]
    fn extract_title_returns_none_when_missing() {
        let html = "<html><body>No title here</body></html>";
        assert_eq!(extract_title(html), None);
    }

    #[test]
    fn extract_title_returns_none_when_empty() {
        let html = "<html><title>  </title></html>";
        assert_eq!(extract_title(html), None);
    }

    #[test]
    fn extract_text_strips_tags() {
        let html = "<p>Hello <b>world</b></p>";
        let text = extract_text(html);
        assert!(text.contains("Hello"));
        assert!(text.contains("world"));
        assert!(!text.contains('<'));
    }

    #[test]
    fn extract_text_removes_script_blocks() {
        let html = "<p>Before</p><script>var x = 1;</script><p>After</p>";
        let text = extract_text(html);
        assert!(text.contains("Before"));
        assert!(text.contains("After"));
        assert!(!text.contains("var x"));
    }

    #[test]
    fn extract_text_removes_style_blocks() {
        let html = "<style>.foo { color: red; }</style><p>Content</p>";
        let text = extract_text(html);
        assert!(text.contains("Content"));
        assert!(!text.contains("color"));
    }

    #[test]
    fn extract_text_decodes_html_entities() {
        let html = "<p>A &amp; B &lt; C &gt; D</p>";
        let text = extract_text(html);
        assert!(text.contains("A & B < C > D"));
    }

    #[test]
    fn extract_text_collapses_whitespace() {
        let html = "<p>  too   many    spaces  </p>";
        let text = extract_text(html);
        assert!(!text.contains("  "));
        assert!(text.contains("too many spaces"));
    }

    #[test]
    fn escalation_on_403_status() {
        assert!(needs_escalation(
            StatusCode::FORBIDDEN,
            "<html>Access denied</html>"
        ));
    }

    #[test]
    fn escalation_on_429_status() {
        assert!(needs_escalation(
            StatusCode::TOO_MANY_REQUESTS,
            "<html>Rate limited</html>"
        ));
    }

    #[test]
    fn escalation_on_503_status() {
        assert!(needs_escalation(
            StatusCode::SERVICE_UNAVAILABLE,
            "<html>Try again</html>"
        ));
    }

    #[test]
    fn escalation_on_short_body_with_noscript() {
        let body = "<html><noscript>Enable JS</noscript></html>";
        assert!(body.len() < MIN_BODY_LENGTH);
        assert!(needs_escalation(StatusCode::OK, body));
    }

    #[test]
    fn no_escalation_on_short_body_without_noscript() {
        let body = "<html><body>Short</body></html>";
        assert!(!needs_escalation(StatusCode::OK, body));
    }

    #[test]
    fn escalation_on_next_js_marker() {
        let body = r#"<html><script id="__NEXT_DATA__">{"props":{}}</script></html>"#;
        assert!(needs_escalation(StatusCode::OK, body));
    }

    #[test]
    fn escalation_on_vue_marker() {
        let body = "<html><div id=\"app\" __vue></div></html>";
        assert!(needs_escalation(StatusCode::OK, body));
    }

    #[test]
    fn escalation_on_angular_marker() {
        let body = "<html><body ng-app=\"myApp\"></body></html>";
        assert!(needs_escalation(StatusCode::OK, body));
    }

    #[test]
    fn escalation_on_react_marker() {
        let body = "<html><div data-reactroot></div></html>";
        assert!(needs_escalation(StatusCode::OK, body));
    }

    #[test]
    fn escalation_on_login_redirect_pattern() {
        let body =
            r#"<html><meta http-equiv="refresh" content="0;url=/login?redirect=foo"></html>"#;
        assert!(needs_escalation(StatusCode::OK, body));
    }

    #[test]
    fn escalation_on_signin_form_action() {
        let body = r#"<html><form action="/signin"><input name="user"/></form></html>"#;
        assert!(needs_escalation(StatusCode::OK, body));
    }

    #[test]
    fn no_escalation_on_normal_page() {
        let body = "<html><head><title>Normal Page</title></head>\
                     <body><p>This is a perfectly normal page with enough content \
                     to pass the minimum body length check. It has no JS framework \
                     markers and no login redirects. Just plain HTML content that \
                     should render fine without a browser engine. Lorem ipsum dolor \
                     sit amet, consectetur adipiscing elit. Sed do eiusmod tempor \
                     incididunt ut labore et dolore magna aliqua. Ut enim ad minim \
                     veniam.</p></body></html>";
        assert!(!needs_escalation(StatusCode::OK, body));
    }

    #[test]
    fn remove_nested_script_tags() {
        let html = "before<script type=\"text/javascript\">alert('hi')</script>after";
        let result = remove_tag_blocks(html, "script");
        assert_eq!(result, "beforeafter");
    }

    #[test]
    fn remove_multiple_style_blocks() {
        let html = "<style>a{}</style>mid<style>b{}</style>end";
        let result = remove_tag_blocks(html, "style");
        assert_eq!(result, "midend");
    }

    #[test]
    fn collapse_multiple_spaces() {
        assert_eq!(collapse_whitespace("a  b   c"), "a b c");
    }

    #[test]
    fn collapse_trims_edges() {
        assert_eq!(collapse_whitespace("  hello  "), "hello");
    }

    #[test]
    fn http_fetcher_builds_successfully() {
        let fetcher = HttpFetcher::new();
        assert!(fetcher.is_ok());
    }

    #[test]
    fn http_fetcher_custom_timeout() {
        let fetcher = HttpFetcher::with_timeout(Duration::from_secs(5));
        assert!(fetcher.is_ok());
    }

    #[test]
    fn fetch_router_builds_successfully() {
        let router = FetchRouter::new();
        assert!(router.is_ok());
    }

    #[test]
    fn http_response_to_page_extracts_fields() {
        let resp = HttpResponse {
            url: "https://example.com".to_string(),
            status: StatusCode::OK,
            body: "<html><title>Example</title><body><p>Hello</p></body></html>".to_string(),
        };
        let page = http_response_to_page(resp);
        assert_eq!(page.url, "https://example.com");
        assert_eq!(page.title, Some("Example".to_string()));
        assert!(page.text.contains("Hello"));
        assert!(page.html.contains("<title>"));
    }

    #[test]
    fn http_response_to_page_missing_title() {
        let resp = HttpResponse {
            url: "https://example.com/notitle".to_string(),
            status: StatusCode::OK,
            body: "<html><body>No title</body></html>".to_string(),
        };
        let page = http_response_to_page(resp);
        assert!(page.title.is_none());
    }

    #[tokio::test]
    #[ignore = "requires network access"]
    async fn http_fetcher_fetch_example_com() {
        let fetcher = HttpFetcher::new().expect("client should build");
        let resp = fetcher
            .fetch("https://example.com")
            .await
            .expect("fetch should succeed");
        assert_eq!(resp.status, StatusCode::OK);
        assert!(resp.body.contains("Example Domain"));
    }

    #[tokio::test]
    #[ignore = "requires network access"]
    async fn fetch_router_no_browser_uses_http() {
        let router = FetchRouter::new().expect("router should build");
        let page = router
            .fetch("https://example.com", None)
            .await
            .expect("fetch should succeed");
        assert!(page.title.is_some());
        assert!(page.text.contains("Example Domain"));
    }
}
