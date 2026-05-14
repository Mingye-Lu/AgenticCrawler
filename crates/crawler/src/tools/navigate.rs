use serde_json::{json, Value};

use crate::browser::BrowserContext;
use crate::fetcher::FetchRouter;
use crate::markdown::{extract_main_html, html_to_markdown, DEFAULT_MAX_MARKDOWN_CHARS};
use crate::tools::page_map::apply_page_map_caps;
use crate::{CrawlError, ToolEffect, ToolError};

const SLIM_MAX_CHARS: usize = 2000;

#[derive(Debug, PartialEq)]
enum ContentDepth {
    Full,
    Main,
    Slim,
    None,
}

#[derive(Debug)]
struct NavigateInput {
    url: String,
    format: String,
    content_depth: ContentDepth,
    strip_images: bool,
}

fn parse_input(input: &Value) -> Result<NavigateInput, CrawlError> {
    let url = input
        .get("url")
        .and_then(Value::as_str)
        .ok_or_else(|| CrawlError::new("missing required field: url"))?;

    if url.is_empty() {
        return Err(CrawlError::new("url must not be empty"));
    }

    // Allowlist http/https only. Without this, the agent could be steered into
    // file://, javascript:, data:, or other schemes that bypass network
    // boundaries (local-file disclosure, SSRF helpers, etc.).
    let scheme_end = url.find(':').ok_or_else(|| {
        CrawlError::new("url must include a scheme (http:// or https://)")
    })?;
    let scheme = &url[..scheme_end];
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return Err(CrawlError::new(
            "url scheme must be http or https",
        ));
    }

    let format = input
        .get("format")
        .and_then(Value::as_str)
        .unwrap_or("markdown");

    if !matches!(format, "markdown" | "text" | "html") {
        return Err(CrawlError::new(
            "format must be one of: markdown, text, html",
        ));
    }

    let content_depth = match input
        .get("content_depth")
        .and_then(Value::as_str)
        .unwrap_or("main")
    {
        "full" => ContentDepth::Full,
        "main" => ContentDepth::Main,
        "slim" => ContentDepth::Slim,
        "none" => ContentDepth::None,
        _ => {
            return Err(CrawlError::new(
                "content_depth must be one of: full, main, slim, none",
            ))
        }
    };

    let strip_images = input
        .get("strip_images")
        .and_then(Value::as_bool)
        .unwrap_or(true);

    Ok(NavigateInput {
        url: url.to_string(),
        format: format.to_string(),
        content_depth,
        strip_images,
    })
}

fn cap_content(content: &str, max_chars: usize) -> (String, bool) {
    if content.chars().count() <= max_chars {
        (content.to_string(), false)
    } else {
        let truncated = content.char_indices().nth(max_chars).map_or_else(
            || content.to_string(),
            |(idx, _)| content[..idx].to_string(),
        );
        (truncated, true)
    }
}

fn strip_markdown_images(md: &str) -> String {
    let mut result = String::with_capacity(md.len());
    let mut chars = md.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '!' && chars.peek() == Some(&'[') {
            chars.next();
            let mut depth = 1;
            let mut found_close = false;
            for c in chars.by_ref() {
                if c == '[' {
                    depth += 1;
                } else if c == ']' {
                    depth -= 1;
                    if depth == 0 {
                        found_close = true;
                        break;
                    }
                }
            }
            if found_close && chars.peek() == Some(&'(') {
                chars.next();
                let mut paren_depth = 1;
                for c in chars.by_ref() {
                    if c == '(' {
                        paren_depth += 1;
                    } else if c == ')' {
                        paren_depth -= 1;
                        if paren_depth == 0 {
                            break;
                        }
                    }
                }
            }
        } else {
            result.push(ch);
        }
    }

    result
}

fn extract_headings_from_markdown(md: &str) -> Value {
    let headings = md
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            let hash_count = trimmed.chars().take_while(|ch| *ch == '#').count();
            if !(1..=6).contains(&hash_count) {
                return None;
            }

            let text = trimmed
                .strip_prefix(&"#".repeat(hash_count))?
                .strip_prefix(' ')?;

            Some(json!({
                "level": hash_count,
                "text": text,
                "id": Value::Null,
                "selector": Value::Null
            }))
        })
        .collect::<Vec<_>>();

    json!({
        "headings": headings,
        "meta": {
            "url": "",
            "title": "",
            "description": ""
        }
    })
}

fn resolve_content(
    html: &str,
    text: &str,
    markdown: &str,
    format: &str,
    depth: &ContentDepth,
) -> (String, bool) {
    if *depth == ContentDepth::None {
        return (String::new(), false);
    }

    let max_chars = match depth {
        ContentDepth::Slim => SLIM_MAX_CHARS,
        _ => std::env::var("ACRAWL_MAX_MARKDOWN_CHARS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(DEFAULT_MAX_MARKDOWN_CHARS),
    };

    match depth {
        ContentDepth::Full => match format {
            "markdown" => cap_content(markdown, max_chars),
            "text" => cap_content(text, max_chars),
            "html" => cap_content(html, max_chars),
            _ => unreachable!(),
        },
        ContentDepth::Main | ContentDepth::Slim => {
            let main_html = extract_main_html(html);
            match format {
                "markdown" => {
                    let md = html_to_markdown(&main_html);
                    cap_content(&md, max_chars)
                }
                "text" => cap_content(text, max_chars),
                "html" => cap_content(&main_html, max_chars),
                _ => unreachable!(),
            }
        }
        ContentDepth::None => unreachable!(),
    }
}

pub async fn execute(input: &Value, browser: &mut BrowserContext) -> Result<ToolEffect, ToolError> {
    let params = parse_input(input)?;

    let router = FetchRouter::new().map_err(|e| ToolError(e.to_string()))?;
    let page = router
        .fetch(&params.url, Some(browser))
        .await
        .map_err(|e| ToolError(e.to_string()))?;

    let title = page.title.clone().unwrap_or_default();

    let (content, truncated) = resolve_content(
        &page.html,
        &page.text,
        &page.markdown,
        &params.format,
        &params.content_depth,
    );

    let content = if params.strip_images && params.format == "markdown" {
        strip_markdown_images(&content)
    } else {
        content
    };

    browser.set_navigated_url(&page.url, page.fetched_via_browser);

    let page_map = if page.fetched_via_browser {
        match browser.acquire_bridge().await {
            Ok(mut bridge) => match bridge.page_map().await {
                Ok(mut value) => {
                    apply_page_map_caps(&mut value);
                    value
                }
                Err(_) => json!({
                    "headings": [],
                    "meta": {
                        "url": page.url.clone(),
                        "title": ""
                    }
                }),
            },
            Err(_) => json!({
                "headings": [],
                "meta": {
                    "url": page.url.clone(),
                    "title": ""
                }
            }),
        }
    } else {
        let mut value = extract_headings_from_markdown(&page.markdown);
        if let Some(meta) = value.get_mut("meta").and_then(Value::as_object_mut) {
            meta.insert("url".to_string(), json!(page.url.clone()));
            meta.insert("title".to_string(), json!(title.clone()));
        }
        value
    };

    let content_length = content.chars().count();

    Ok(ToolEffect::reply_json(&json!({
        "url": page.url,
        "title": title,
        "content": content,
        "format": params.format,
        "content_depth": match params.content_depth {
            ContentDepth::Full => "full",
            ContentDepth::Main => "main",
            ContentDepth::Slim => "slim",
            ContentDepth::None => "none",
        },
        "truncated": truncated,
        "content_length": content_length,
        "page_map": page_map
    })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown::html_to_markdown_capped;
    use serde_json::json;

    #[test]
    fn navigate_parse_format_defaults_to_markdown() {
        let input = json!({"url": "https://example.com"});
        let result = parse_input(&input).unwrap();
        assert_eq!(result.url, "https://example.com");
        assert_eq!(result.format, "markdown");
    }

    #[test]
    fn navigate_parse_format_accepts_text_html_markdown() {
        for format in ["text", "html", "markdown"] {
            let input = json!({"url": "https://example.com", "format": format});
            let result = parse_input(&input).unwrap();
            assert_eq!(result.format, format);
        }
    }

    #[test]
    fn navigate_parse_format_rejects_invalid() {
        let input = json!({"url": "https://example.com", "format": "pdf"});
        let err = parse_input(&input).unwrap_err();
        assert!(err.to_string().contains("format"));
    }

    #[test]
    fn navigate_parse_content_depth_defaults_to_main() {
        let input = json!({"url": "https://example.com"});
        let result = parse_input(&input).unwrap();
        assert_eq!(result.content_depth, ContentDepth::Main);
    }

    #[test]
    fn navigate_parse_content_depth_all_values() {
        for (val, expected) in [
            ("full", ContentDepth::Full),
            ("main", ContentDepth::Main),
            ("slim", ContentDepth::Slim),
            ("none", ContentDepth::None),
        ] {
            let input = json!({"url": "https://x.com", "content_depth": val});
            let result = parse_input(&input).unwrap();
            assert_eq!(result.content_depth, expected);
        }
    }

    #[test]
    fn navigate_parse_rejects_non_http_schemes() {
        for url in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "data:text/html,<h1>x</h1>",
            "ftp://example.com/foo",
            "noscheme",
        ] {
            let input = json!({"url": url});
            let err = parse_input(&input).expect_err(&format!("expected error for {url}"));
            let msg = err.to_string();
            assert!(
                msg.contains("scheme") || msg.contains("http"),
                "unexpected error for {url}: {msg}"
            );
        }
    }

    #[test]
    fn navigate_parse_accepts_http_and_https_case_insensitively() {
        for url in ["http://example.com", "HTTPS://Example.com/path"] {
            let input = json!({"url": url});
            parse_input(&input).unwrap_or_else(|e| panic!("rejected {url}: {e}"));
        }
    }

    #[test]
    fn navigate_parse_content_depth_rejects_invalid() {
        let input = json!({"url": "https://x.com", "content_depth": "deep"});
        let err = parse_input(&input).unwrap_err();
        assert!(err.to_string().contains("content_depth"));
    }

    #[test]
    fn resolve_content_none_returns_empty() {
        let (content, truncated) = resolve_content(
            "<p>hello</p>",
            "hello",
            "hello",
            "markdown",
            &ContentDepth::None,
        );
        assert!(content.is_empty());
        assert!(!truncated);
    }

    #[test]
    fn resolve_content_main_extracts_article() {
        let html =
            r"<nav>Menu</nav><main><h1>Title</h1><p>Body text</p></main><footer>Footer</footer>";
        let md = html_to_markdown(html);
        let (content, _) = resolve_content(html, "text", &md, "markdown", &ContentDepth::Main);
        assert!(content.contains("Title"));
        assert!(content.contains("Body text"));
        assert!(!content.contains("Menu"));
        assert!(!content.contains("Footer"));
    }

    #[test]
    fn resolve_content_full_includes_everything() {
        let html = r"<header><p>Header</p></header><main><p>Body</p></main>";
        let md = html_to_markdown(html);
        let (content, _) = resolve_content(html, "text", &md, "markdown", &ContentDepth::Full);
        assert!(content.contains("Header"));
        assert!(content.contains("Body"));
    }

    #[test]
    fn resolve_content_slim_caps_at_2000() {
        let body = "a".repeat(5000);
        let html = format!("<main><p>{body}</p></main>");
        let md = html_to_markdown(&html);
        let (content, truncated) =
            resolve_content(&html, "text", &md, "markdown", &ContentDepth::Slim);
        assert!(truncated);
        assert!(content.chars().count() <= SLIM_MAX_CHARS);
    }

    #[test]
    fn navigate_response_has_new_shape() {
        let result = extract_headings_from_markdown("# Title\n\n## Section");
        assert!(result.get("headings").is_some());
        assert!(result.get("meta").is_some());
        assert!(result["meta"].get("url").is_some());
        assert!(result["meta"].get("title").is_some());
        assert!(result["meta"].get("description").is_some());
    }

    #[test]
    fn navigate_markdown_content_has_structure() {
        let (content, truncated) = html_to_markdown_capped(
            r#"<h1>Docs</h1><p>Read the <a href="https://example.com/guide">guide</a>.</p>"#,
            DEFAULT_MAX_MARKDOWN_CHARS,
        );
        assert!(!truncated);
        assert!(content.contains("# Docs"));
        assert!(content.contains("[guide](https://example.com/guide)"));
    }

    #[test]
    fn navigate_text_format_backward_compat() {
        let input = "a".repeat(33_000);
        let (content, truncated) = cap_content(&input, 32_000);
        assert!(truncated);
        assert_eq!(content.chars().count(), 32_000);
    }

    #[test]
    fn navigate_truncation_flag_set_on_large_content() {
        let html = format!("<p>{}</p>", "a".repeat(DEFAULT_MAX_MARKDOWN_CHARS + 256));
        let (_content, truncated) = html_to_markdown_capped(&html, DEFAULT_MAX_MARKDOWN_CHARS);
        assert!(truncated);
    }

    #[test]
    fn navigate_page_map_included_in_response() {
        let result = extract_headings_from_markdown("# Title");
        assert_eq!(result["headings"].as_array().map(Vec::len), Some(1));
        assert!(result["meta"].is_object());
    }

    #[test]
    fn extract_headings_from_markdown_finds_all_levels() {
        let result = extract_headings_from_markdown("# H1\n## H2\n### H3");
        let headings = result["headings"].as_array().unwrap();
        assert_eq!(headings.len(), 3);
        assert_eq!(headings[0]["level"], 1);
        assert_eq!(headings[0]["text"], "H1");
        assert_eq!(headings[1]["level"], 2);
        assert_eq!(headings[1]["text"], "H2");
        assert_eq!(headings[2]["level"], 3);
        assert_eq!(headings[2]["text"], "H3");
    }

    #[test]
    fn strip_images_removes_markdown_images() {
        let md = "Before ![alt text](https://example.com/img.png) after";
        let stripped = strip_markdown_images(md);
        assert_eq!(stripped, "Before  after");
    }

    #[test]
    fn strip_images_handles_nested_brackets() {
        let md = "Text ![complex [alt]](url) more";
        let stripped = strip_markdown_images(md);
        assert_eq!(stripped, "Text  more");
    }

    #[test]
    fn strip_images_preserves_regular_links() {
        let md = "Click [here](https://example.com) to continue";
        let stripped = strip_markdown_images(md);
        assert_eq!(stripped, md);
    }

    #[test]
    fn parse_strip_images_defaults_to_true() {
        let input = json!({"url": "https://x.com"});
        let result = parse_input(&input).unwrap();
        assert!(result.strip_images);
    }

    #[test]
    fn parse_strip_images_can_be_disabled() {
        let input = json!({"url": "https://x.com", "strip_images": false});
        let result = parse_input(&input).unwrap();
        assert!(!result.strip_images);
    }
}
