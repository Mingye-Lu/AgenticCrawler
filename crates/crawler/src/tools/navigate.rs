use serde_json::Value;

use crate::browser::BrowserContext;
use crate::fetcher::FetchRouter;
use crate::{CrawlError, ToolEffect, ToolError};

#[derive(Debug)]
struct NavigateInput {
    url: String,
}

fn parse_input(input: &Value) -> Result<NavigateInput, CrawlError> {
    let url = input
        .get("url")
        .and_then(Value::as_str)
        .ok_or_else(|| CrawlError::new("missing required field: url"))?;

    if url.is_empty() {
        return Err(CrawlError::new("url must not be empty"));
    }

    Ok(NavigateInput {
        url: url.to_string(),
    })
}

fn truncate_html(html: &str, max_chars: usize) -> String {
    let char_count = html.chars().count();
    if char_count <= max_chars {
        html.to_string()
    } else {
        let truncated: String = html.chars().take(max_chars).collect();
        format!("{truncated}...")
    }
}

pub async fn execute(input: &Value, browser: &mut BrowserContext) -> Result<ToolEffect, ToolError> {
    let params = parse_input(input)?;

    let router = FetchRouter::new().map_err(|e| ToolError(e.to_string()))?;
    let page = router
        .fetch(&params.url, Some(browser))
        .await
        .map_err(|e| ToolError(e.to_string()))?;

    Ok(ToolEffect::reply_json(&serde_json::json!({
        "title": page.title.unwrap_or_default(),
        "url": page.url,
        "text": truncate_html(&page.text, 4000),
        "html_summary": truncate_html(&page.html, 2000)
    })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_valid_url() {
        let input = json!({"url": "https://example.com"});
        let result = parse_input(&input);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().url, "https://example.com");
    }

    #[test]
    fn parse_missing_url_returns_error() {
        let input = json!({});
        let err = parse_input(&input).unwrap_err();
        assert!(err.to_string().contains("url"));
    }

    #[test]
    fn parse_empty_url_returns_error() {
        let input = json!({"url": ""});
        let err = parse_input(&input).unwrap_err();
        assert!(err.to_string().contains("empty"));
    }

    #[test]
    fn parse_non_string_url_returns_error() {
        let input = json!({"url": 42});
        assert!(parse_input(&input).is_err());
    }

    #[test]
    fn truncate_html_short_text_unchanged() {
        assert_eq!(truncate_html("short", 100), "short");
    }

    #[test]
    fn truncate_html_long_text_is_truncated() {
        let long = "a".repeat(3000);
        let result = truncate_html(&long, 2000);
        assert!(result.ends_with("..."));
        assert_eq!(result.len(), 2003);
    }

    #[test]
    fn truncate_html_exact_boundary() {
        let exact = "x".repeat(2000);
        assert_eq!(truncate_html(&exact, 2000), exact);
    }
}
