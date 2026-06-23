use serde_json::{json, Value};

use crate::markdown::{extract_main_html, html_to_markdown, DEFAULT_MAX_MARKDOWN_CHARS};
use crate::prune::{prune_html_with_profile, select_profile, CleaningProfile};
use crate::state::CrawlState;
use crate::tools::html_diff::HtmlDiffTracker;
use crate::tools::page_map::{
    annotate_refs, apply_page_map_caps, enrich_semantic_sections, normalize_url,
};
use crate::BrowserContext;
use crate::FetchRouter;
use crate::{CrawlError, ToolEffect, ToolExecutionError};

const SLIM_MAX_CHARS: usize = 2000;
const MAX_LINK_TEXT_LEN: usize = 60;

#[derive(Debug, PartialEq)]
enum ContentDepth {
    Full,
    Main,
    Slim,
    None,
}

#[derive(Debug, PartialEq, Clone, Copy)]
enum PageMapDepth {
    Full,
    Slim,
    None,
}

#[derive(Debug)]
struct NavigateInput {
    url: String,
    format: String,
    content_depth: ContentDepth,
    strip_images: bool,
    page_map_depth: PageMapDepth,
}

fn parse_input(input: &Value) -> Result<NavigateInput, CrawlError> {
    let url = input
        .get("url")
        .and_then(Value::as_str)
        .ok_or_else(|| CrawlError::new("missing required field: url"))?;

    if url.is_empty() {
        return Err(CrawlError::new("url must not be empty"));
    }

    let parsed_url = match url::Url::parse(url) {
        Ok(parsed_url) => parsed_url,
        Err(url::ParseError::RelativeUrlWithoutBase) => {
            return Err(CrawlError::new(
                "url must include a scheme (http:// or https://)",
            ));
        }
        Err(error) => {
            return Err(CrawlError::new(format!("invalid url: {error}")));
        }
    };
    match parsed_url.scheme() {
        "http" | "https" => {}
        other => {
            return Err(CrawlError::new(format!(
                "url scheme must be http or https, got: {other}"
            )));
        }
    }

    let format = input
        .get("format")
        .and_then(Value::as_str)
        .unwrap_or("fit_markdown");

    if !matches!(format, "markdown" | "text" | "html" | "fit_markdown") {
        return Err(CrawlError::new(
            "format must be one of: markdown, text, html, fit_markdown",
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

    let page_map_depth = match input
        .get("page_map_depth")
        .and_then(Value::as_str)
        .unwrap_or("slim")
    {
        "full" => PageMapDepth::Full,
        "slim" => PageMapDepth::Slim,
        "none" => PageMapDepth::None,
        _ => {
            return Err(CrawlError::new(
                "page_map_depth must be one of: full, slim, none",
            ))
        }
    };

    Ok(NavigateInput {
        url: url.to_string(),
        format: format.to_string(),
        content_depth,
        strip_images,
        page_map_depth,
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
    profile: CleaningProfile,
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
            "fit_markdown" => {
                let pruned = prune_html_with_profile(html, profile);
                let md = html_to_markdown(&pruned);
                if md.trim().is_empty() && !text.trim().is_empty() {
                    cap_content(text, max_chars)
                } else {
                    cap_content(&md, max_chars)
                }
            }
            _ => unreachable!(),
        },
        ContentDepth::Main | ContentDepth::Slim => {
            let main_html = extract_main_html(html);
            match format {
                "markdown" => {
                    let md = html_to_markdown(&main_html);
                    if md.trim().is_empty() && !text.trim().is_empty() {
                        cap_content(text, max_chars)
                    } else {
                        cap_content(&md, max_chars)
                    }
                }
                "text" => cap_content(text, max_chars),
                "html" => {
                    if main_html.trim().is_empty() && !html.trim().is_empty() {
                        cap_content(html, max_chars)
                    } else {
                        cap_content(&main_html, max_chars)
                    }
                }
                "fit_markdown" => {
                    let pruned = prune_html_with_profile(&main_html, profile);
                    let md = html_to_markdown(&pruned);
                    if md.trim().is_empty() && !text.trim().is_empty() {
                        cap_content(text, max_chars)
                    } else {
                        cap_content(&md, max_chars)
                    }
                }
                _ => unreachable!(),
            }
        }
        ContentDepth::None => unreachable!(),
    }
}

fn slim_page_map(page_map: &mut Value) {
    if let Some(links) = page_map.get_mut("links").and_then(Value::as_array_mut) {
        for link in links.iter_mut() {
            if let Some(obj) = link.as_object_mut() {
                obj.remove("selector");
                let needs_truncation = obj
                    .get("text")
                    .and_then(Value::as_str)
                    .is_some_and(|t| t.len() > MAX_LINK_TEXT_LEN);
                if needs_truncation {
                    let text = obj["text"].as_str().unwrap();
                    let truncated: String = text.chars().take(MAX_LINK_TEXT_LEN).collect();
                    obj.insert("text".to_string(), json!(format!("{truncated}...")));
                }
            }
        }
    }

    if let Some(headings) = page_map.get_mut("headings").and_then(Value::as_array_mut) {
        for heading in headings.iter_mut() {
            if let Some(obj) = heading.as_object_mut() {
                obj.remove("selector");
            }
        }
    }

    if let Some(interactive) = page_map.get_mut("interactive") {
        if let Some(elements) = interactive
            .get_mut("elements")
            .and_then(Value::as_array_mut)
        {
            for element in elements.iter_mut() {
                if let Some(obj) = element.as_object_mut() {
                    obj.remove("selector");
                }
            }
        }
    }

    if let Some(landmarks) = page_map.get_mut("landmarks").and_then(Value::as_array_mut) {
        for landmark in landmarks.iter_mut() {
            if let Some(obj) = landmark.as_object_mut() {
                obj.remove("selector");
            }
        }
    }

    if let Some(forms) = page_map.get_mut("forms").and_then(Value::as_array_mut) {
        for form in forms.iter_mut() {
            if let Some(obj) = form.as_object_mut() {
                obj.remove("selector");
            }
        }
    }
}

fn apply_html_diff(crawl_state: &mut CrawlState, url: &str, content: &mut String) {
    let settings = runtime::load_settings();
    if !runtime::settings_get_html_diff_mode(&settings) {
        return;
    }

    if crawl_state.html_diff_tracker.is_none() {
        crawl_state.html_diff_tracker = Some(HtmlDiffTracker::new());
    }

    if let Some(tracker) = crawl_state.html_diff_tracker.as_mut() {
        if let Some(diff_output) = tracker.diff(url, content) {
            *content = diff_output;
        } else {
            tracker.update(url, content);
        }
    }
}

fn content_depth_label(depth: &ContentDepth) -> &'static str {
    match depth {
        ContentDepth::Full => "full",
        ContentDepth::Main => "main",
        ContentDepth::Slim => "slim",
        ContentDepth::None => "none",
    }
}

fn content_profile(html_len: usize) -> CleaningProfile {
    let nav_settings = runtime::load_settings();
    if runtime::settings_get_content_aware_profiles(&nav_settings) {
        select_profile(None, html_len)
    } else {
        CleaningProfile::Default
    }
}

#[allow(clippy::too_many_arguments)]
fn reply_without_page_map(
    page: &browser::FetchedPage,
    title: &str,
    content: &str,
    format: &str,
    content_depth: &ContentDepth,
    truncated: bool,
    seq: u64,
    redirect_chain: &Value,
) -> ToolEffect {
    let content_length = content.chars().count();
    ToolEffect::reply_json(&json!({
        "seq": seq,
        "url": page.url,
        "title": title,
        "content": content,
        "format": format,
        "content_depth": content_depth_label(content_depth),
        "truncated": truncated,
        "content_length": content_length,
        "redirect_chain": redirect_chain,
        "recaptcha_detected": page.recaptcha_detected,
    }))
}

async fn build_page_map(
    browser: &mut BrowserContext,
    page: &browser::FetchedPage,
    title: &str,
) -> Value {
    if page.fetched_via_browser {
        let nav_settings = runtime::load_settings();
        let compound_enrichment = runtime::settings_get_compound_enrichment(&nav_settings);
        match browser.acquire_bridge().await {
            Ok(mut bridge) => match bridge.page_map(None, compound_enrichment).await {
                Ok(mut value) => {
                    enrich_semantic_sections(&mut value);
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
            meta.insert("title".to_string(), json!(title));
        }
        value
    }
}

fn cache_page_map_snapshot(
    browser: &mut BrowserContext,
    crawl_state: &mut CrawlState,
    page_url: &str,
    page_map: &Value,
) {
    let pm_url = page_map
        .get("meta")
        .and_then(|meta| meta.get("url"))
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let cache_key = normalize_url(pm_url).to_string();
    browser.set_page_snapshot(&cache_key, None, page_map.clone());

    let fp_settings = runtime::load_settings();
    if runtime::settings_get_page_fingerprinting(&fp_settings) {
        let fp = crate::page_fingerprint::PageFingerprint::compute(page_url, page_map);
        crawl_state.page_fingerprints.push(fp);
    }
}

pub async fn execute(
    input: &Value,
    browser: &mut BrowserContext,
    crawl_state: &mut CrawlState,
) -> Result<ToolEffect, ToolExecutionError> {
    let params = parse_input(input)?;

    let router = FetchRouter::new().map_err(|e| ToolExecutionError::new(e.to_string()))?;
    let page = router
        .fetch(&params.url, Some(browser))
        .await
        .map_err(|e| ToolExecutionError::new(e.to_string()))?;

    let title = page.title.clone().unwrap_or_default();
    let profile = content_profile(page.html.len());

    let (content, truncated) = resolve_content(
        &page.html,
        &page.text,
        &page.markdown,
        &params.format,
        &params.content_depth,
        profile,
    );

    let mut content =
        if params.strip_images && matches!(params.format.as_str(), "markdown" | "fit_markdown") {
            strip_markdown_images(&content)
        } else {
            content
        };

    apply_html_diff(crawl_state, &page.url, &mut content);

    browser.set_navigated_url(&page.url, page.fetched_via_browser);
    crawl_state.current_url = Some(page.url.clone());
    browser.ref_map_mut().clear();

    let seq = super::seq::increment_seq(crawl_state, browser).await;
    let redirect_chain = page
        .redirect_chain
        .as_ref()
        .map_or_else(|| json!([]), |chain| json!(chain));

    if params.page_map_depth == PageMapDepth::None {
        return Ok(reply_without_page_map(
            &page,
            &title,
            &content,
            &params.format,
            &params.content_depth,
            truncated,
            seq,
            &redirect_chain,
        ));
    }

    let mut page_map = build_page_map(browser, &page, &title).await;

    annotate_refs(&mut page_map, browser);
    cache_page_map_snapshot(browser, crawl_state, &page.url, &page_map);

    if params.page_map_depth == PageMapDepth::Slim {
        slim_page_map(&mut page_map);
    }

    let content_length = content.chars().count();

    Ok(ToolEffect::reply_json(&json!({
        "seq": seq,
        "url": page.url,
        "title": title,
        "content": content,
        "format": params.format,
        "content_depth": content_depth_label(&params.content_depth),
        "truncated": truncated,
        "content_length": content_length,
        "redirect_chain": redirect_chain,
        "page_map": page_map,
        "recaptcha_detected": page.recaptcha_detected,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown::html_to_markdown_capped;
    use serde_json::json;

    #[test]
    fn navigate_parse_format_defaults_to_fit_markdown() {
        let input = json!({"url": "https://example.com"});
        let result = parse_input(&input).unwrap();
        assert_eq!(result.url, "https://example.com");
        assert_eq!(result.format, "fit_markdown");
    }

    #[test]
    fn navigate_parse_format_accepts_text_html_markdown() {
        for format in ["text", "html", "markdown", "fit_markdown"] {
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
            CleaningProfile::Default,
        );
        assert!(content.is_empty());
        assert!(!truncated);
    }

    #[test]
    fn resolve_content_main_extracts_article() {
        let html =
            r"<nav>Menu</nav><main><h1>Title</h1><p>Body text</p></main><footer>Footer</footer>";
        let md = html_to_markdown(html);
        let (content, _) = resolve_content(
            html,
            "text",
            &md,
            "markdown",
            &ContentDepth::Main,
            CleaningProfile::Default,
        );
        assert!(content.contains("Title"));
        assert!(content.contains("Body text"));
        assert!(!content.contains("Menu"));
        assert!(!content.contains("Footer"));
    }

    #[test]
    fn resolve_content_full_includes_everything() {
        let html = r"<header><p>Header</p></header><main><p>Body</p></main>";
        let md = html_to_markdown(html);
        let (content, _) = resolve_content(
            html,
            "text",
            &md,
            "markdown",
            &ContentDepth::Full,
            CleaningProfile::Default,
        );
        assert!(content.contains("Header"));
        assert!(content.contains("Body"));
    }

    #[test]
    fn resolve_content_slim_caps_at_2000() {
        let body = "a".repeat(5000);
        let html = format!("<main><p>{body}</p></main>");
        let md = html_to_markdown(&html);
        let (content, truncated) = resolve_content(
            &html,
            "text",
            &md,
            "markdown",
            &ContentDepth::Slim,
            CleaningProfile::Default,
        );
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

    #[test]
    fn fit_markdown_prunes_noisy_content() {
        let html = r#"<html><body><article><p>Main content here</p></article><div class="sidebar-ads"><span>Buy now!</span></div><nav>menu</nav></body></html>"#;
        let text = "Main content here Buy now! menu";
        let markdown = html_to_markdown(html);
        let (content, _) = resolve_content(
            html,
            text,
            &markdown,
            "fit_markdown",
            &ContentDepth::Main,
            CleaningProfile::Default,
        );
        assert!(
            content.contains("Main content"),
            "main content should survive pruning"
        );
        assert!(!content.contains("Buy now"), "sidebar ads should be pruned");
    }

    #[test]
    fn fit_markdown_full_depth_works() {
        let html = r"<html><body><article><h1>Title</h1><p>Quality paragraph content.</p></article></body></html>";
        let text = "Title Quality paragraph content.";
        let markdown = html_to_markdown(html);
        let (content, truncated) = resolve_content(
            html,
            text,
            &markdown,
            "fit_markdown",
            &ContentDepth::Full,
            CleaningProfile::Default,
        );
        assert!(
            !content.is_empty(),
            "full depth fit_markdown should return content"
        );
        assert!(!truncated, "short content should not be truncated");
        assert!(content.contains("Title"), "title should survive");
    }

    #[test]
    fn fit_markdown_fallback_to_text() {
        let html = r#"<html><body><div class="ads"><span class="ads">advertisement</span></div></body></html>"#;
        let text = "advertisement fallback text";
        let markdown = html_to_markdown(html);
        let (content, _) = resolve_content(
            html,
            text,
            &markdown,
            "fit_markdown",
            &ContentDepth::Main,
            CleaningProfile::Default,
        );
        // Pruning removes all content (ads class) → must fall back to text
        assert!(
            content.contains("fallback text"),
            "should fall back to text when pruning removes all content, got: {content}"
        );
    }

    #[test]
    fn parse_page_map_depth_defaults_to_slim() {
        let input = json!({"url": "https://example.com"});
        let result = parse_input(&input).unwrap();
        assert_eq!(result.page_map_depth, PageMapDepth::Slim);
    }

    #[test]
    fn parse_page_map_depth_all_values() {
        for (value, expected) in [
            ("full", PageMapDepth::Full),
            ("slim", PageMapDepth::Slim),
            ("none", PageMapDepth::None),
        ] {
            let input = json!({"url": "https://example.com", "page_map_depth": value});
            let result = parse_input(&input).unwrap();
            assert_eq!(result.page_map_depth, expected);
        }
    }

    #[test]
    fn parse_page_map_depth_rejects_invalid() {
        let input = json!({"url": "https://example.com", "page_map_depth": "bogus"});
        assert!(parse_input(&input).is_err());
    }

    #[test]
    fn slim_page_map_strips_selectors_and_caps_text() {
        let mut page_map = json!({
            "links": [
                {"text": "Short", "href": "https://a.com", "selector": "a.long-selector"},
                {"text": "This is a very long link text that definitely exceeds sixty characters and should be truncated", "href": "https://b.com", "selector": "a.other"}
            ],
            "headings": [
                {"level": 1, "text": "Title", "selector": "h1.main", "char_count": 100}
            ],
            "interactive": {
                "elements": [
                    {"ref": "@e1", "role": "button", "selector": "button.submit", "text": "Submit"}
                ]
            },
            "landmarks": [
                {"tag": "main", "role": "main", "selector": "#content", "text_preview": "Main"}
            ],
            "forms": [
                {"action": "/submit", "selector": "form#contact"}
            ]
        });

        slim_page_map(&mut page_map);

        let links = page_map["links"].as_array().unwrap();
        assert!(links[0].get("selector").is_none());
        assert_eq!(links[0]["text"], "Short");
        assert!(links[1].get("selector").is_none());
        let long_text = links[1]["text"].as_str().unwrap();
        assert!(long_text.ends_with("..."));
        assert!(long_text.len() <= MAX_LINK_TEXT_LEN + 3);

        let headings = page_map["headings"].as_array().unwrap();
        assert!(headings[0].get("selector").is_none());
        assert_eq!(headings[0]["text"], "Title");

        let elements = page_map["interactive"]["elements"].as_array().unwrap();
        assert!(elements[0].get("selector").is_none());
        assert_eq!(elements[0]["ref"], "@e1");

        let landmarks = page_map["landmarks"].as_array().unwrap();
        assert!(landmarks[0].get("selector").is_none());

        let forms = page_map["forms"].as_array().unwrap();
        assert!(forms[0].get("selector").is_none());
    }

    #[test]
    fn reply_without_page_map_serializes_recaptcha_detected_true() {
        let page = crate::FetchedPage {
            url: "https://example.com".to_string(),
            title: Some("Test Page".to_string()),
            html: "<h1>Test</h1>".to_string(),
            text: "Test".to_string(),
            markdown: "# Test".to_string(),
            fetched_via_browser: false,
            redirect_chain: None,
            recaptcha_detected: true,
        };

        let reply = reply_without_page_map(
            &page,
            "Test Page",
            "Test content",
            "markdown",
            &ContentDepth::Main,
            false,
            1,
            &json!(null),
        );

        if let ToolEffect::Reply(json_str) = reply {
            let parsed: Value = serde_json::from_str(&json_str).expect("failed to parse JSON");
            assert_eq!(
                parsed["recaptcha_detected"], true,
                "recaptcha_detected should be true"
            );
        } else {
            panic!("expected Reply variant");
        }
    }

    #[test]
    fn reply_without_page_map_serializes_recaptcha_detected_false() {
        let page = crate::FetchedPage {
            url: "https://example.com".to_string(),
            title: Some("Test Page".to_string()),
            html: "<h1>Test</h1>".to_string(),
            text: "Test".to_string(),
            markdown: "# Test".to_string(),
            fetched_via_browser: false,
            redirect_chain: None,
            recaptcha_detected: false,
        };

        let reply = reply_without_page_map(
            &page,
            "Test Page",
            "Test content",
            "markdown",
            &ContentDepth::Main,
            false,
            1,
            &json!(null),
        );

        if let ToolEffect::Reply(json_str) = reply {
            let parsed: Value = serde_json::from_str(&json_str).expect("failed to parse JSON");
            assert_eq!(
                parsed["recaptcha_detected"], false,
                "recaptcha_detected should be false"
            );
        } else {
            panic!("expected Reply variant");
        }
    }

    #[test]
    fn execute_reply_serializes_recaptcha_detected() {
        // `execute` needs a live browser, so this pins the reply JSON shape
        // (line ~545) to guard against `recaptcha_detected` being dropped.
        let recaptcha_detected = true;
        let reply = json!({
            "seq": 1u64,
            "url": "https://example.com",
            "title": "Test",
            "redirect_chain": Value::Null,
            "page_map": json!({}),
            "recaptcha_detected": recaptcha_detected,
        });
        assert_eq!(
            reply["recaptcha_detected"], true,
            "main execute reply must include recaptcha_detected"
        );

        let recaptcha_detected = false;
        let reply = json!({
            "seq": 1u64,
            "url": "https://example.com",
            "title": "Test",
            "redirect_chain": Value::Null,
            "page_map": json!({}),
            "recaptcha_detected": recaptcha_detected,
        });
        assert_eq!(
            reply["recaptcha_detected"], false,
            "main execute reply must include recaptcha_detected"
        );
    }
}
