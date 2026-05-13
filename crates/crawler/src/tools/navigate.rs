use serde_json::{json, Value};

use crate::browser::BrowserContext;
use crate::fetcher::FetchRouter;
use crate::markdown::DEFAULT_MAX_MARKDOWN_CHARS;
use crate::tools::page_map::apply_page_map_caps;
use crate::{CrawlError, ToolEffect, ToolError};

#[derive(Debug)]
struct NavigateInput {
    url: String,
    format: String,
}

fn parse_input(input: &Value) -> Result<NavigateInput, CrawlError> {
    let url = input
        .get("url")
        .and_then(Value::as_str)
        .ok_or_else(|| CrawlError::new("missing required field: url"))?;

    if url.is_empty() {
        return Err(CrawlError::new("url must not be empty"));
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

    Ok(NavigateInput {
        url: url.to_string(),
        format: format.to_string(),
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

pub async fn execute(input: &Value, browser: &mut BrowserContext) -> Result<ToolEffect, ToolError> {
    let params = parse_input(input)?;

    let router = FetchRouter::new().map_err(|e| ToolError(e.to_string()))?;
    let page = router
        .fetch(&params.url, Some(browser))
        .await
        .map_err(|e| ToolError(e.to_string()))?;

    let title = page.title.clone().unwrap_or_default();
    let (content, truncated) = match params.format.as_str() {
        "markdown" => cap_content(&page.markdown, DEFAULT_MAX_MARKDOWN_CHARS),
        "text" => cap_content(&page.text, 32_000),
        "html" => cap_content(&page.html, 32_000),
        _ => unreachable!("parse_input validates format"),
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
}
