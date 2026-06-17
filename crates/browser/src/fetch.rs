use std::fmt;
use std::time::Duration;

use reqwest::header::{HeaderMap, HeaderValue, CONTENT_TYPE, USER_AGENT};
use reqwest::{Client, StatusCode};

use crate::BrowserContext;
use crate::PageInfo;

#[derive(Debug, Clone)]
pub struct FetchedPage {
    pub url: String,
    pub title: Option<String>,
    pub html: String,
    pub text: String,
    pub markdown: String,
    pub fetched_via_browser: bool,
}

#[derive(Debug)]
pub enum FetchError {
    Http(reqwest::Error),
    Browser(String),
    StatusError {
        status: u16,
        url: String,
    },
    /// Response body exceeded the configured maximum size.
    BodyTooLarge {
        url: String,
        limit: usize,
    },
}

impl fmt::Display for FetchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Http(e) => write!(f, "HTTP fetch error: {e}"),
            Self::Browser(msg) => write!(f, "Browser fetch error: {msg}"),
            Self::StatusError { status, url } => {
                write!(f, "HTTP {status} for {url}")
            }
            Self::BodyTooLarge { url, limit } => {
                write!(f, "response body for {url} exceeds {limit} bytes")
            }
        }
    }
}

impl std::error::Error for FetchError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Http(e) => Some(e),
            Self::Browser(_) | Self::StatusError { .. } | Self::BodyTooLarge { .. } => None,
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

/// Minimum HTML size before we bother running structural analysis.
const MIN_HTML_FOR_SPA_CHECK: usize = 100;

/// Minimum visible text length — pages below this threshold get a structural
/// score boost. Also used by the Playwright bridge script's hydration wait.
const MIN_VISIBLE_CHARS_THRESHOLD: usize = 200;

/// Score threshold for SPA shell detection. Signals are accumulated and
/// compared against this value.
const SPA_SHELL_SCORE_THRESHOLD: f32 = 0.60;

/// Detect whether an HTTP response body looks like an empty SPA/CSR shell that
/// needs browser rendering to produce meaningful content.
///
/// Uses multi-signal scoring rather than a single binary check:
/// - **Negative signals** (data already present → return false immediately):
///   `__NEXT_DATA__`, `window.__NUXT__`, `data-server-rendered`, `data-reactroot`
/// - **High-weight signals**: framework asset paths without embedded data,
///   Angular empty root, noscript "enable JavaScript" messages
/// - **Medium-weight signals**: empty mount-point divs, Vite/CRA bundle hashes
/// - **Low-weight structural signals**: sparse visible text, absence of semantic
///   HTML elements (`<h1>`, `<article>`, `<main>`, `<p>`)
///
/// Escalates to browser only when accumulated score ≥ 0.60, which requires
/// at least one framework/structural signal beyond just "low text content".
fn looks_like_empty_spa_shell(body: &str) -> bool {
    if body.len() < MIN_HTML_FOR_SPA_CHECK {
        return false;
    }

    let lower = body.to_ascii_lowercase();

    // ── Negative signals: data already embedded, no browser needed ──
    // These indicate SSR/SSG worked — content is in the HTML already.
    if lower.contains("__next_data__")
        || lower.contains("window.__nuxt__")
        || lower.contains("window.__nuxt_data__")
        || lower.contains("window.__remixcontext")
        || lower.contains("data-server-rendered=\"true\"")
        || lower.contains("data-reactroot")
        || lower.contains("data-reactid")
    {
        return false;
    }

    let mut score: f32 = 0.0;

    // ── High: framework asset paths WITHOUT embedded data (already excluded above) ──
    if lower.contains("/_next/static/") {
        score += 0.70;
    }
    if lower.contains("/_nuxt/") {
        score += 0.65;
    }
    if lower.contains("ng-version=") {
        score += 0.80;
    }
    if lower.contains("data-sveltekit") || lower.contains("__sveltekit") {
        score += 0.55;
    }

    // ── High: noscript "enable JavaScript" messages (Vue CLI / CRA template) ──
    if lower.contains("enable javascript")
        || lower.contains("doesn't work properly without javascript")
        || lower.contains("requires javascript")
    {
        score += 0.55;
    }

    // ── Medium: empty mount-point divs (framework root with no children) ──
    if lower.contains("id=\"root\"></div>")
        || lower.contains("id=\"app\"></div>")
        || lower.contains("id=\"__next\"></div>")
        || lower.contains("id=\"__nuxt\"></div>")
        || lower.contains("<app-root></app-root>")
        || lower.contains("<app-root />")
    {
        score += 0.45;
    }

    // ── Medium: bundler hash patterns (Vite/CRA output) ──
    // Match patterns like: src="/assets/index-D9LVtTP6.js" or
    // src="/static/js/main.abc123.chunk.js"
    if has_bundler_hash_pattern(&lower) {
        score += 0.30;
    }

    // ── Low: structural signals (modifiers, not sufficient alone) ──
    if body.len() > 2000 {
        let visible_len = extract_text(body).trim().len();
        if visible_len < MIN_VISIBLE_CHARS_THRESHOLD {
            score += 0.35;
        }

        let has_semantic = lower.contains("<h1")
            || lower.contains("<article")
            || lower.contains("<main>")
            || lower.contains("<p>");
        if !has_semantic {
            score += 0.20;
        }
    }

    score >= SPA_SHELL_SCORE_THRESHOLD
}

/// Check for Vite/Rollup/Webpack hash patterns in script src attributes.
/// These patterns (`index-XXXXXXXX.js`, `main.XXXXXXXX.chunk.js`) are
/// characteristic of bundled SPA builds.
#[allow(clippy::case_sensitive_file_extension_comparisons)] // input is pre-lowercased
fn has_bundler_hash_pattern(lower_html: &str) -> bool {
    for segment in lower_html.split("src=\"") {
        if let Some(path) = segment.split('"').next() {
            if (path.ends_with(".js") || path.ends_with(".mjs")) && has_hash_segment(path) {
                return true;
            }
        }
    }
    false
}

/// Returns true if the path contains a segment of 8+ alphanumeric characters
/// (with at least 2 digits) immediately preceded by `-` or `.`.
/// Matches Vite (base62), Webpack/CRA (hex) hash patterns.
fn has_hash_segment(path: &str) -> bool {
    let bytes = path.as_bytes();
    let len = bytes.len();
    let mut i = 0;
    while i < len {
        if bytes[i] == b'-' || bytes[i] == b'.' {
            let start = i + 1;
            let mut j = start;
            let mut digit_count = 0u32;
            while j < len && bytes[j].is_ascii_alphanumeric() {
                if bytes[j].is_ascii_digit() {
                    digit_count += 1;
                }
                j += 1;
            }
            let seg_len = j - start;
            // 8+ alphanumeric chars with at least 2 digits distinguishes
            // hashes (e.g. "d9lvttp6") from English words (e.g. "loader")
            if seg_len >= 8 && digit_count >= 2 && j < len && (bytes[j] == b'.' || bytes[j] == b'/') {
                return true;
            }
            i = j;
        } else {
            i += 1;
        }
    }
    false
}


/// Hard cap on a single HTTP response body. A page that is much larger than
/// this is almost never useful to the agent (and is almost certainly a binary
/// dump, generated content, or an attack) — and without a cap, reqwest's
/// `.text()` will happily buffer multi-gigabyte responses into memory.
const MAX_BODY_BYTES: usize = 32 * 1024 * 1024;

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

fn extract_charset(content_type: &str) -> Option<&str> {
    content_type.split(';').find_map(|part| {
        let part = part.trim();
        if part.to_ascii_lowercase().starts_with("charset=") {
            Some(part[8..].trim_matches('"').trim())
        } else {
            None
        }
    })
}

fn decode_body_with_charset(bytes: &[u8], charset: &str) -> String {
    if charset.eq_ignore_ascii_case("utf-8") || charset.eq_ignore_ascii_case("us-ascii") {
        if let Ok(s) = String::from_utf8(bytes.to_vec()) {
            if !looks_like_misinterpreted_gbk(&s) {
                return s;
            }
        }
        // Bytes are either invalid UTF-8 or valid UTF-8 that looks like
        // GBK misinterpreted as UTF-8 (common with Chinese APIs that omit
        // Content-Type). Try GBK decoding.
        let (decoded, _, had_errors) = encoding_rs::GBK.decode(bytes);
        if !had_errors {
            return decoded.into_owned();
        }
        return String::from_utf8_lossy(bytes).into_owned();
    }
    let encoding =
        encoding_rs::Encoding::for_label(charset.as_bytes()).unwrap_or(encoding_rs::UTF_8);
    let (decoded, _, _) = encoding.decode(bytes);
    decoded.into_owned()
}

/// GBK byte pairs (0x81-0xFE lead, 0x40-0xFE trail) that happen to be valid
/// UTF-8 typically produce characters in Armenian (U+0530-058F), Georgian
/// (U+10A0-10FF), or modifier ranges. If the non-ASCII portion of a string
/// is dominated by these scripts (and no actual CJK), the bytes are almost
/// certainly GBK misread as UTF-8.
fn looks_like_misinterpreted_gbk(text: &str) -> bool {
    let mut suspicious = 0u32;
    let mut total_non_ascii = 0u32;
    for ch in text.chars() {
        if !ch.is_ascii() {
            total_non_ascii += 1;
            let cp = ch as u32;
            if (0x0080..=0x024F).contains(&cp)      // Latin Extended / IPA
                || (0x0370..=0x03FF).contains(&cp)   // Greek
                || (0x0400..=0x04FF).contains(&cp)   // Cyrillic
                || (0x0530..=0x058F).contains(&cp)   // Armenian
                || (0x10A0..=0x10FF).contains(&cp)
            // Georgian
            {
                suspicious += 1;
            }
        }
    }
    total_non_ascii >= 4 && suspicious * 2 > total_non_ascii
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
        let mut resp = self.client.get(url).send().await?;
        let status = resp.status();
        let final_url = resp.url().to_string();

        let charset = resp
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .and_then(extract_charset)
            .unwrap_or("utf-8")
            .to_string();

        // Reject up front if the server advertises a body over the limit so we
        // don't even start downloading the payload.
        if let Some(declared) = resp.content_length() {
            if declared > MAX_BODY_BYTES as u64 {
                return Err(FetchError::BodyTooLarge {
                    url: final_url,
                    limit: MAX_BODY_BYTES,
                });
            }
        }

        // Stream the body chunk-by-chunk so a missing or lying Content-Length
        // header can still be caught mid-transfer.
        let mut bytes: Vec<u8> = Vec::new();
        while let Some(chunk) = resp.chunk().await? {
            if bytes.len() + chunk.len() > MAX_BODY_BYTES {
                return Err(FetchError::BodyTooLarge {
                    url: final_url,
                    limit: MAX_BODY_BYTES,
                });
            }
            bytes.extend_from_slice(&chunk);
        }

        let body = decode_body_with_charset(&bytes, &charset);

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
                if looks_like_empty_spa_shell(&resp.body) {
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
            .map_err(|e| FetchError::Browser(e.to_string()))?
            .navigate(url)
            .await
            .map_err(|e| FetchError::Browser(e.to_string()))?;

        let text = extract_text(&page_info.html);
        let markdown = crate::markdown::html_to_markdown(&page_info.html);
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
            markdown,
            fetched_via_browser: true,
        })
    }
}

fn http_response_to_page(resp: HttpResponse) -> FetchedPage {
    let title = extract_title(&resp.body);
    let text = extract_text(&resp.body);
    let markdown = crate::markdown::html_to_markdown(&resp.body);
    FetchedPage {
        url: resp.url,
        title,
        html: resp.body,
        text,
        markdown,
        fetched_via_browser: false,
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

    #[test]
    fn http_response_to_page_has_markdown_field() {
        let resp = HttpResponse {
            url: "https://example.com".to_string(),
            status: StatusCode::OK,
            body: "<html><body><p>Hello world</p></body></html>".to_string(),
        };
        let page = http_response_to_page(resp);
        assert!(!page.markdown.is_empty());
    }

    #[test]
    fn http_response_to_page_markdown_has_structure() {
        let resp = HttpResponse {
            url: "https://example.com".to_string(),
            status: StatusCode::OK,
            body: "<html><body><h1>Title</h1><a href=\"/link\">Click</a></body></html>".to_string(),
        };
        let page = http_response_to_page(resp);
        assert!(
            page.markdown.contains("# "),
            "expected heading marker in markdown"
        );
        assert!(
            page.markdown.contains('['),
            "expected link syntax in markdown"
        );
    }

    #[test]
    fn http_response_to_page_non_html_skips_markdown() {
        let resp = HttpResponse {
            url: "https://api.example.com/data".to_string(),
            status: StatusCode::OK,
            body: r#"{"key": "value"}"#.to_string(),
        };
        let page = http_response_to_page(resp);
        assert!(
            page.markdown.starts_with("```"),
            "non-HTML input should be wrapped in a code block"
        );
    }

    #[tokio::test]
    async fn fetch_rejects_oversized_content_length() {
        use tokio::io::AsyncWriteExt;
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");

        // Serve a single connection that advertises a body well over the cap
        // and then closes — we expect the client to bail out before reading
        // anywhere near that many bytes.
        let server = tokio::spawn(async move {
            if let Ok((mut socket, _)) = listener.accept().await {
                use tokio::io::AsyncReadExt;
                // Drain the request so Windows doesn't RST the connection
                // before the client can read the response headers.
                let mut buf = [0u8; 4096];
                let _ = socket.read(&mut buf).await;
                let _ = socket
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 999999999\r\nConnection: close\r\n\r\n",
                    )
                    .await;
                // Let the socket drop naturally; an immediate shutdown() sends a
                // TCP RST on Windows, aborting the client read before it sees
                // the Content-Length header.
                tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
            }
        });

        let fetcher = HttpFetcher::new().expect("client");
        let url = format!("http://{addr}/");
        let err = fetcher
            .fetch(&url)
            .await
            .expect_err("oversized body must be rejected");
        let _ = server.await;

        match err {
            FetchError::BodyTooLarge { limit, .. } => {
                assert_eq!(limit, MAX_BODY_BYTES);
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn http_response_to_page_markdown_field_exists_on_struct() {
        let page = FetchedPage {
            url: String::new(),
            title: None,
            html: String::new(),
            text: String::new(),
            markdown: String::new(),
            fetched_via_browser: false,
        };
        // Compile-time check: the markdown field exists and is a String
        let _: &String = &page.markdown;
    }

    #[test]
    fn spa_shell_detected_next_js_csr() {
        // Next.js CSR: has /_next/static/ assets but no __NEXT_DATA__ blob
        let body = r#"<html><head></head><body>
            <div id="__next"></div>
            <script src="/_next/static/chunks/main-a1b2c3d4.js"></script>
            <script src="/_next/static/chunks/pages/_app-e5f6a7b8.js"></script>
        </body></html>"#;
        assert!(looks_like_empty_spa_shell(body));
    }

    #[test]
    fn spa_shell_not_triggered_next_js_ssr() {
        // Next.js SSR: has __NEXT_DATA__ → data is embedded, no browser needed
        let body = r#"<html><head></head><body>
            <div id="__next"><h1>Server Rendered Page</h1><p>Content here</p></div>
            <script id="__NEXT_DATA__" type="application/json">{"props":{}}</script>
            <script src="/_next/static/chunks/main-a1b2c3d4.js"></script>
        </body></html>"#;
        assert!(!looks_like_empty_spa_shell(body));
    }

    #[test]
    fn spa_shell_detected_angular_empty_root() {
        let body = r#"<html><head></head><body>
            <app-root ng-version="17.3.0"></app-root>
            <script src="/main.a1b2c3d4e5f6.js"></script>
        </body></html>"#;
        assert!(looks_like_empty_spa_shell(body));
    }

    #[test]
    fn spa_shell_detected_vue_cli_noscript() {
        let body = r#"<html><head></head><body>
            <noscript><strong>We're sorry but this app doesn't work properly without JavaScript enabled.</strong></noscript>
            <div id="app"></div>
            <script src="/js/app.a1b2c3d4.js"></script>
            <script src="/js/chunk-vendors.e5f6a7b8.js"></script>
        </body></html>"#;
        assert!(looks_like_empty_spa_shell(body));
    }

    #[test]
    fn spa_shell_detected_empty_root_with_vite_bundle() {
        // Generic Vite SPA: empty #root + hashed bundle
        let body = r#"<html><head></head><body>
            <div id="root"></div>
            <script type="module" src="/assets/index-D9LVtTP6.js"></script>
        </body></html>"#;
        assert!(looks_like_empty_spa_shell(body));
    }

    #[test]
    fn spa_shell_not_triggered_contentful_page() {
        let mut body = String::from("<html><body><main>");
        for _ in 0..50 {
            body.push_str("<p>This is a paragraph with meaningful content for users.</p>");
        }
        body.push_str("</main></body></html>");
        assert!(!looks_like_empty_spa_shell(&body));
    }

    #[test]
    fn spa_shell_not_triggered_nuxt_ssr() {
        // Nuxt SSR: has window.__NUXT__ → data present
        let body = r#"<html><head></head><body>
            <div id="__nuxt"><div id="__layout"><h1>Hello</h1></div></div>
            <script>window.__NUXT__={data:{},state:{}}</script>
        </body></html>"#;
        assert!(!looks_like_empty_spa_shell(body));
    }

    #[test]
    fn spa_shell_not_triggered_react_ssr() {
        // React SSR: has data-reactroot → content was server-rendered
        let body = r#"<html><head></head><body>
            <div id="root" data-reactroot=""><h1>Hello</h1><p>Content</p></div>
        </body></html>"#;
        assert!(!looks_like_empty_spa_shell(body));
    }

    #[test]
    fn spa_shell_skips_small_pages() {
        let body = "<html><body>OK</body></html>";
        assert!(body.len() < MIN_HTML_FOR_SPA_CHECK);
        assert!(!looks_like_empty_spa_shell(body));
    }

    #[test]
    fn spa_shell_not_triggered_sparse_but_legitimate_page() {
        // A sparse login page: has <p> and <h1>, no framework signals
        let body = r#"<html><head><title>Login</title></head><body>
            <h1>Sign In</h1>
            <form action="/login" method="post">
                <input type="email" name="email" />
                <input type="password" name="pass" />
                <button type="submit">Log in</button>
            </form>
            <p>Forgot your password?</p>
        </body></html>"#;
        assert!(!looks_like_empty_spa_shell(body));
    }

    #[test]
    fn spa_shell_low_text_alone_not_sufficient() {
        // Large HTML with scripts but no framework signals — should NOT trigger
        // because low text alone (score 0.35) is below the 0.60 threshold
        let mut body = String::from("<html><body><div>");
        for _ in 0..200 {
            body.push_str("<script>var x = 1;</script>");
        }
        body.push_str("<p>tiny</p></div></body></html>");
        assert!(body.len() > MIN_HTML_FOR_SPA_CHECK);
        assert!(!looks_like_empty_spa_shell(&body));
    }

    #[test]
    fn bundler_hash_detection_vite() {
        assert!(has_hash_segment("/assets/index-a1b2c3d4.js"));
        assert!(has_hash_segment("/assets/vendor-e5f6a7b8c9d0.js"));
    }

    #[test]
    fn bundler_hash_detection_no_false_positive() {
        assert!(!has_hash_segment("/js/app.js"));
        assert!(!has_hash_segment("/main.js"));
        assert!(!has_hash_segment("/assets/short-abc.js"));
    }
}
