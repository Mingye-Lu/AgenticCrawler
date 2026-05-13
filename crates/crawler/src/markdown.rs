use htmd::HtmlToMarkdown;
use std::sync::LazyLock;

static CONVERTER: LazyLock<HtmlToMarkdown> = LazyLock::new(|| {
    HtmlToMarkdown::builder()
        .skip_tags(vec!["script", "style", "noscript", "iframe", "head", "svg"])
        .build()
});

pub const DEFAULT_MAX_MARKDOWN_CHARS: usize = 32_000;

pub fn html_to_markdown(html: &str) -> String {
    if is_non_html_input(html) {
        return wrap_in_code_block(html);
    }

    let markdown = CONVERTER.convert(html).unwrap_or_else(|_| html.to_string());

    normalize_markdown(&markdown)
}

#[must_use]
pub fn html_to_markdown_capped(html: &str, max_chars: usize) -> (String, bool) {
    let markdown = html_to_markdown(html);
    let max_chars = std::env::var("ACRAWL_MAX_MARKDOWN_CHARS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(max_chars);

    truncate_markdown(markdown, max_chars)
}

fn is_non_html_input(input: &str) -> bool {
    let trimmed = input.trim_start();

    trimmed.starts_with('{')
        || trimmed.starts_with('[')
        || trimmed.starts_with("%PDF")
        || contains_binary_controls(trimmed)
}

fn contains_binary_controls(input: &str) -> bool {
    input
        .chars()
        .any(|ch| ch.is_control() && !matches!(ch, '\n' | '\r' | '\t'))
}

fn wrap_in_code_block(content: &str) -> String {
    format!("```\n{content}\n```")
}

fn normalize_markdown(markdown: &str) -> String {
    let mut normalized = String::with_capacity(markdown.len());
    let mut in_code_block = false;

    for line in markdown.lines() {
        if line.starts_with("```") {
            in_code_block = !in_code_block;
            normalized.push_str(line);
        } else if !in_code_block {
            if let Some(rest) = line.strip_prefix("* ") {
                normalized.push_str("- ");
                normalized.push_str(rest.trim_start());
            } else {
                normalized.push_str(line);
            }
        } else {
            normalized.push_str(line);
        }

        normalized.push('\n');
    }

    if !markdown.ends_with('\n') && !normalized.is_empty() {
        normalized.pop();
    }

    normalized
}

fn truncate_markdown(markdown: String, max_chars: usize) -> (String, bool) {
    if markdown.chars().count() <= max_chars {
        return (markdown, false);
    }

    let truncated = markdown
        .char_indices()
        .nth(max_chars)
        .map_or(markdown.clone(), |(idx, _)| markdown[..idx].to_string());

    (truncated, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_to_markdown_preserves_headings() {
        let markdown = html_to_markdown("<h1>Title</h1>");
        assert!(markdown.contains("# Title"));
    }

    #[test]
    fn html_to_markdown_preserves_links() {
        let markdown = html_to_markdown("<a href=\"https://example.com\">text</a>");
        assert!(markdown.contains("[text](https://example.com)"));
    }

    #[test]
    fn html_to_markdown_preserves_lists() {
        let markdown = html_to_markdown("<ul><li>item</li></ul>");
        assert!(markdown.contains("- item"));
    }

    #[test]
    fn html_to_markdown_preserves_code_blocks() {
        let markdown = html_to_markdown("<pre><code>fn main()</code></pre>");
        assert!(markdown.contains("```"));
        assert!(markdown.contains("fn main()"));
    }

    #[test]
    fn html_to_markdown_preserves_tables() {
        let html =
            "<table><tr><th>Name</th><th>Value</th></tr><tr><td>foo</td><td>bar</td></tr></table>";
        let markdown = html_to_markdown(html);
        assert!(markdown.contains('|'));
        assert!(markdown.contains("Name"));
        assert!(markdown.contains("foo"));
    }

    #[test]
    fn html_to_markdown_strips_script_style() {
        let html = "<script>alert('x')</script><style>body{color:red;}</style><p>content</p>";
        let markdown = html_to_markdown(html);
        assert!(markdown.contains("content"));
        assert!(!markdown.contains("alert"));
        assert!(!markdown.contains("color:red"));
    }

    #[test]
    fn html_to_markdown_handles_malformed_html() {
        let markdown = html_to_markdown("<div><h1>Title<p>body");
        assert!(markdown.contains("Title"));
        assert!(markdown.contains("body"));
    }

    #[test]
    fn html_to_markdown_capped_truncates() {
        let html = format!("<p>{}</p>", "a".repeat(DEFAULT_MAX_MARKDOWN_CHARS + 256));
        let (markdown, was_truncated) = html_to_markdown_capped(&html, DEFAULT_MAX_MARKDOWN_CHARS);
        assert_eq!(markdown.chars().count(), DEFAULT_MAX_MARKDOWN_CHARS);
        assert!(was_truncated);
    }

    #[test]
    fn html_to_markdown_capped_no_truncate_small() {
        let (markdown, was_truncated) = html_to_markdown_capped("<p>short text</p>", 128);
        assert!(!was_truncated);
        assert!(markdown.contains("short text"));
    }

    #[test]
    fn html_to_markdown_non_html_passthrough() {
        let markdown = html_to_markdown("{\"key\": \"value\"}");
        assert_eq!(markdown, "```\n{\"key\": \"value\"}\n```");
    }

    #[test]
    fn html_to_markdown_real_page_structure() {
        let html = r#"
            <html>
                <head><title>Example</title></head>
                <body>
                    <header>
                        <h1>Docs</h1>
                        <p>Welcome to the docs.</p>
                    </header>
                    <main>
                        <section>
                            <h2>Getting Started</h2>
                            <p>Read the <a href="https://example.com/guide">guide</a>.</p>
                        </section>
                        <section>
                            <h2>Reference</h2>
                            <ul>
                                <li>API</li>
                                <li>Examples</li>
                            </ul>
                        </section>
                    </main>
                </body>
            </html>
        "#;

        let markdown = html_to_markdown(html);
        assert!(markdown.contains("# Docs"));
        assert!(markdown.contains("## Getting Started"));
        assert!(markdown.contains("[guide](https://example.com/guide)"));
    }
}
