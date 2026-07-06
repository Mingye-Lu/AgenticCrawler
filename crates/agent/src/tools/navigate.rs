use serde_json::{json, Value};

use crate::aria::{assign_refs, parse_raw_tree, to_yaml, AriaNode, AriaStates};
use crate::markdown::{extract_main_html, html_to_markdown, DEFAULT_MAX_MARKDOWN_CHARS};
use crate::page_fingerprint::PageFingerprint;
use crate::prune::{prune_html_with_profile, select_profile, CleaningProfile};
use crate::state::CrawlState;
use crate::tools::html_diff::HtmlDiffTracker;
use crate::tools::page_map::normalize_url;
use crate::BrowserContext;
use crate::FetchRouter;
use crate::{CrawlError, ToolEffect, ToolExecutionError};

const SLIM_MAX_CHARS: usize = 2000;
/// ARIA-tree serialization depth for navigate's structural section. Kept equal
/// to the `page_map` tool default so both surfaces emit comparable trees.
const NAVIGATE_TREE_DEPTH: usize = 5;

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

fn synthesize_tree_from_markdown(markdown: &str) -> AriaNode {
    let mut children = Vec::new();
    for line in markdown.lines() {
        let trimmed = line.trim_start();
        let level = trimmed.chars().take_while(|ch| *ch == '#').count();
        if !(1..=6).contains(&level) {
            continue;
        }
        let Some(rest) = trimmed.strip_prefix(&"#".repeat(level)) else {
            continue;
        };
        let Some(rest) = rest.strip_prefix(' ') else {
            continue;
        };
        let text = rest.trim().to_string();
        if text.is_empty() {
            continue;
        }
        children.push(AriaNode {
            role: "heading".to_string(),
            name: Some(text),
            states: AriaStates {
                level: u8::try_from(level).ok(),
                ..AriaStates::default()
            },
            ref_id: None,
            url: None,
            frame_id: None,
            offscreen: false,
            children: Vec::new(),
            omitted_children: 0,
        });
    }

    AriaNode {
        role: "document".to_string(),
        name: None,
        states: AriaStates::default(),
        ref_id: None,
        url: None,
        frame_id: None,
        offscreen: false,
        children,
        omitted_children: 0,
    }
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
    }))
}

async fn fetch_aria_tree(browser: &mut BrowserContext) -> Option<AriaNode> {
    let nav_settings = runtime::load_settings();
    let compound_enrichment = runtime::settings_get_compound_enrichment(&nav_settings);
    let result = browser
        .acquire_bridge()
        .await
        .ok()?
        .page_map(None, compound_enrichment, None)
        .await
        .ok()?;
    parse_raw_tree(result.get("tree")?)
}

async fn build_structural_yaml(
    browser: &mut BrowserContext,
    crawl_state: &mut CrawlState,
    page: &browser::FetchedPage,
) -> String {
    let mut tree = if page.fetched_via_browser {
        fetch_aria_tree(browser)
            .await
            .unwrap_or_else(|| synthesize_tree_from_markdown(&page.markdown))
    } else {
        synthesize_tree_from_markdown(&page.markdown)
    };

    if page.fetched_via_browser {
        browser.ref_map_mut().begin_snapshot();
        assign_refs(
            &mut tree,
            browser.ref_map_mut(),
            None,
            &mut Vec::new(),
            None,
        );
        let cache_key = normalize_url(&page.url).to_string();
        browser.set_page_snapshot(&cache_key, None, json!({ "meta": { "url": page.url } }));
    }

    let fp_settings = runtime::load_settings();
    if runtime::settings_get_page_fingerprinting(&fp_settings) {
        let fingerprint = PageFingerprint::compute(&page.url, &tree);
        crawl_state.page_fingerprints.push(fingerprint);
    }

    crawl_state.last_aria_tree = Some(tree.clone());

    to_yaml(&tree, Some(NAVIGATE_TREE_DEPTH))
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

    let page_map = build_structural_yaml(browser, crawl_state, &page).await;

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
        "page_map": page_map
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
    fn synthesize_tree_from_markdown_builds_heading_outline() {
        let tree = synthesize_tree_from_markdown("# Title\n\nbody\n\n## Section\n###NoSpace");
        assert_eq!(tree.role, "document");
        assert_eq!(tree.children.len(), 2);
        assert_eq!(tree.children[0].role, "heading");
        assert_eq!(tree.children[0].name.as_deref(), Some("Title"));
        assert_eq!(tree.children[0].states.level, Some(1));
        assert_eq!(tree.children[1].name.as_deref(), Some("Section"));
        assert_eq!(tree.children[1].states.level, Some(2));

        let yaml = to_yaml(&tree, Some(NAVIGATE_TREE_DEPTH));
        assert!(yaml.contains("heading \"Title\" [level=1]"));
        assert!(yaml.contains("heading \"Section\" [level=2]"));
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
}
