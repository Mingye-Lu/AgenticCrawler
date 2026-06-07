use scraper::{node::Node, ElementRef, Html, Selector};
use std::fmt::Write;
use std::sync::LazyLock;

static BODY_SELECTOR: LazyLock<Option<Selector>> = LazyLock::new(|| Selector::parse("body").ok());

const THRESHOLD: f64 = 0.48;
const NEGATIVE_PATTERNS: [&str; 10] = [
    "nav", "footer", "header", "sidebar", "ads", "comment", "promo", "advert", "social", "share",
];
const VOID_ELEMENTS: [&str; 14] = [
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source",
    "track", "wbr",
];

#[must_use]
pub fn prune_html(html: &str) -> String {
    if html.trim().is_empty() {
        return String::new();
    }

    let Some(body_selector) = BODY_SELECTOR.as_ref() else {
        return String::new();
    };

    let document = Html::parse_document(html);
    let Some(body) = document.select(body_selector).next() else {
        return String::new();
    };

    body.children()
        .filter_map(ElementRef::wrap)
        .filter_map(prune_node)
        .collect::<String>()
}

fn prune_node(element: ElementRef<'_>) -> Option<String> {
    if has_negative_class_id_pattern(element) {
        return None;
    }

    if score_element(element) < THRESHOLD {
        return None;
    }

    if is_void_element(element.value().name()) {
        return Some(element.html());
    }

    let tag_name = element.value().name();
    let attrs = build_attributes(element);
    let inner = element
        .children()
        .filter_map(|child| match child.value() {
            Node::Text(text) => Some(escape_text(text.as_ref())),
            Node::Element(_) => ElementRef::wrap(child).and_then(prune_node),
            _ => None,
        })
        .collect::<String>();

    Some(format!("<{tag_name}{attrs}>{inner}</{tag_name}>"))
}

fn score_element(element: ElementRef<'_>) -> f64 {
    let text_len = normalized_text_len(element);
    let tag_len = element.inner_html().len();
    let link_text_len = direct_link_text_len(element);
    let text_density = if tag_len > 0 {
        usize_to_f64(text_len) / usize_to_f64(tag_len)
    } else {
        0.0
    };
    let link_density_complement = if text_len > 0 {
        1.0 - (usize_to_f64(link_text_len) / usize_to_f64(text_len)).min(1.0)
    } else {
        0.0
    };
    let class_id_score = class_id_score(element);
    let ln_term = if text_len > 0 {
        (usize_to_f64(text_len) + 1.0).ln()
    } else {
        0.0
    };

    0.4 * text_density
        + 0.2 * link_density_complement
        + 0.2 * tag_weight(element.value().name())
        + 0.1 * f64::max(0.0, class_id_score)
        + 0.1 * ln_term
}

fn normalized_text_len(element: ElementRef<'_>) -> usize {
    element.text().collect::<String>().trim().len()
}

fn direct_link_text_len(element: ElementRef<'_>) -> usize {
    element
        .children()
        .filter_map(ElementRef::wrap)
        .filter(|child| child.value().name() == "a")
        .map(|child| child.text().collect::<String>().trim().len())
        .sum()
}

fn class_id_score(element: ElementRef<'_>) -> f64 {
    [element.value().attr("class"), element.value().attr("id")]
        .into_iter()
        .flatten()
        .map(str::to_ascii_lowercase)
        .map(|value| {
            usize_to_f64(
                NEGATIVE_PATTERNS
                    .iter()
                    .filter(|pattern| value.contains(**pattern))
                    .count(),
            ) * -0.5
        })
        .sum()
}

fn has_negative_class_id_pattern(element: ElementRef<'_>) -> bool {
    [element.value().attr("class"), element.value().attr("id")]
        .into_iter()
        .flatten()
        .map(str::to_ascii_lowercase)
        .any(|value| {
            NEGATIVE_PATTERNS
                .iter()
                .any(|pattern| value.contains(pattern))
        })
}

fn tag_weight(tag: &str) -> f64 {
    if matches!(tag, "div" | "li" | "ul" | "ol") {
        return 0.5;
    }

    match tag {
        "article" => 1.5,
        "h1" => 1.2,
        "h2" => 1.1,
        "h3" | "p" | "section" => 1.0,
        "h4" => 0.9,
        "h5" | "table" => 0.8,
        "h6" => 0.7,
        "span" => 0.3,
        _ => 0.5,
    }
}

fn is_void_element(tag: &str) -> bool {
    VOID_ELEMENTS.contains(&tag)
}

fn build_attributes(element: ElementRef<'_>) -> String {
    let mut attrs = String::new();

    for (key, value) in element.value().attrs() {
        let _ = write!(attrs, r#" {key}="{}""#, escape_attr(value));
    }

    attrs
}

fn usize_to_f64(value: usize) -> f64 {
    f64::from(u32::try_from(value).unwrap_or(u32::MAX))
}

fn escape_attr(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn escape_text(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn first_body_child_score(html: &str) -> f64 {
        let document = Html::parse_document(html);
        let body_selector = Selector::parse("body").expect("body selector should parse");
        let body = document
            .select(&body_selector)
            .next()
            .expect("body should exist in test html");
        let element = body
            .children()
            .find_map(ElementRef::wrap)
            .expect("body should have a child element");

        score_element(element)
    }

    #[test]
    fn score_calculation_correct() {
        let result = prune_html("<html><body><p>Hello World</p></body></html>");
        assert!(!result.is_empty(), "p tag with content should survive");
        assert!(result.contains("Hello World"));
    }

    #[test]
    fn threshold_boundary() {
        let low_html = r#"<html><body><div class="nav footer ads"><br></div></body></html>"#;
        let high_html = r"<html><body><div>abc</div></body></html>";
        let low_score = first_body_child_score(low_html);
        let high_score = first_body_child_score(high_html);

        assert!(
            low_score < THRESHOLD,
            "expected low score below threshold, got {low_score}"
        );
        assert!(
            high_score >= THRESHOLD,
            "expected high score above threshold, got {high_score}"
        );
        assert!(prune_html(high_html).contains("abc"));
        assert_eq!(prune_html(low_html), "");
    }

    #[test]
    fn negative_class_pattern() {
        let html = r#"<html><body>
            <div class="main-content"><p>content</p></div>
            <div class="sidebar-nav"><p>content</p></div>
        </body></html>"#;
        let result = prune_html(html);

        assert!(result.contains(r#"<div class="main-content"><p>content</p></div>"#));
        assert!(!result.contains("sidebar-nav"));
    }

    #[test]
    fn edge_cases() {
        assert_eq!(prune_html(""), "");
        assert_eq!(
            prune_html(
                r#"<html><body><div class="ads"><span class="ads">x</span></div></body></html>"#
            ),
            ""
        );
    }
}
