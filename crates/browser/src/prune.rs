use scraper::{node::Node, ElementRef, Html, Selector};
use std::fmt::Write;
use std::sync::LazyLock;

static BODY_SELECTOR: LazyLock<Option<Selector>> = LazyLock::new(|| Selector::parse("body").ok());
static ANCHOR_SELECTOR: LazyLock<Option<Selector>> = LazyLock::new(|| Selector::parse("a").ok());

const NEGATIVE_PATTERNS: [&str; 10] = [
    "nav", "footer", "header", "sidebar", "ads", "comment", "promo", "advert", "social", "share",
];
const VOID_ELEMENTS: [&str; 14] = [
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source",
    "track", "wbr",
];

/// Cleaning profile that controls pruning aggressiveness and tag weight behavior.
///
/// Used when the `content_aware_profiles` optimization flag is enabled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CleaningProfile {
    /// Current default behavior — threshold 0.48.
    #[default]
    Default,
    /// Only remove obvious junk — threshold 0.20. Preserves interactive elements.
    Minimal,
    /// Heavy pruning for large pages — threshold 0.65.
    Aggressive,
    /// Article extraction — threshold 0.45, boosted article/paragraph weights.
    ReadingMode,
}

impl CleaningProfile {
    /// Score threshold below which elements are pruned.
    #[must_use]
    pub fn threshold(self) -> f64 {
        match self {
            Self::Default => 0.48,
            Self::Minimal => 0.20,
            Self::Aggressive => 0.65,
            Self::ReadingMode => 0.45,
        }
    }

    /// Extra weight multiplier for a given HTML tag (applied on top of base weights).
    #[must_use]
    pub fn tag_weight_multiplier(self, tag: &str) -> f64 {
        match self {
            Self::ReadingMode => match tag {
                "article" | "main" => 2.0,
                "p" | "h1" | "h2" | "h3" | "h4" => 1.5,
                "nav" | "aside" | "footer" | "header" => 0.1,
                _ => 1.0,
            },
            Self::Minimal => match tag {
                "form" | "input" | "button" | "select" | "textarea" | "label" => 2.0,
                _ => 1.0,
            },
            Self::Default | Self::Aggressive => 1.0,
        }
    }
}

/// Select a cleaning profile based on an optional task hint string and content length.
///
/// Used when the `content_aware_profiles` flag is ON.
#[must_use]
pub fn select_profile(task_hint: Option<&str>, content_len: usize) -> CleaningProfile {
    if content_len > 50_000 {
        return CleaningProfile::Aggressive;
    }

    let Some(hint) = task_hint else {
        return CleaningProfile::Default;
    };
    let h = hint.to_lowercase();

    if h.contains("extract")
        || h.contains("scrape")
        || h.contains("get data")
        || h.contains("read")
        || h.contains("article")
    {
        CleaningProfile::ReadingMode
    } else if h.contains("fill")
        || h.contains("click")
        || h.contains("interact")
        || h.contains("submit")
        || h.contains("form")
    {
        CleaningProfile::Minimal
    } else {
        CleaningProfile::Default
    }
}

#[must_use]
pub fn prune_html(html: &str) -> String {
    prune_html_with_profile(html, CleaningProfile::Default)
}

/// Prune HTML using a specific cleaning profile.
///
/// The profile controls the score threshold and tag weight multipliers.
#[must_use]
pub fn prune_html_with_profile(html: &str, profile: CleaningProfile) -> String {
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
        .filter_map(|el| prune_node(el, profile))
        .collect::<String>()
}

fn prune_node(element: ElementRef<'_>, profile: CleaningProfile) -> Option<String> {
    if has_negative_class_id_pattern(element) {
        return None;
    }

    if score_element(element, profile) < profile.threshold() {
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
            Node::Element(_) => ElementRef::wrap(child).and_then(|el| prune_node(el, profile)),
            _ => None,
        })
        .collect::<String>();

    Some(format!("<{tag_name}{attrs}>{inner}</{tag_name}>"))
}

fn score_element(element: ElementRef<'_>, profile: CleaningProfile) -> f64 {
    let text: String = element.text().collect();
    let text_len = text.trim().len();
    let tag_len = element.inner_html().len();
    let link_text_len = descendant_link_text_len(element);
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

    let base_tag_weight = tag_weight(element.value().name());
    let adjusted_tag_weight =
        base_tag_weight * profile.tag_weight_multiplier(element.value().name());

    0.4 * text_density
        + 0.2 * link_density_complement
        + 0.2 * adjusted_tag_weight
        + 0.1 * f64::max(0.0, class_id_score)
        + 0.1 * ln_term
}

fn descendant_link_text_len(element: ElementRef<'_>) -> usize {
    let Some(selector) = ANCHOR_SELECTOR.as_ref() else {
        return 0;
    };
    element
        .select(selector)
        .map(|anchor| anchor.text().collect::<String>().trim().len())
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
        first_body_child_score_with_profile(html, CleaningProfile::Default)
    }

    fn first_body_child_score_with_profile(html: &str, profile: CleaningProfile) -> f64 {
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

        score_element(element, profile)
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
        let threshold = CleaningProfile::Default.threshold();

        assert!(
            low_score < threshold,
            "expected low score below threshold, got {low_score}"
        );
        assert!(
            high_score >= threshold,
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

    #[test]
    fn nested_link_sidebar_pruned() {
        let html = r#"<html><body>
            <div class="menu"><ul><li><a href="/a">Link A</a></li><li><a href="/b">Link B</a></li><li><a href="/c">Link C</a></li></ul></div>
            <article><p>This is a substantial paragraph of real content that should survive pruning.</p></article>
        </body></html>"#;
        let result = prune_html(html);
        assert!(
            result.contains("substantial paragraph"),
            "article content should survive"
        );
        assert!(
            !result.contains("Link A"),
            "nested link-heavy sidebar should be pruned"
        );
    }

    #[test]
    fn default_profile_matches_prune_html() {
        let html = r#"<html><body>
            <article><p>This is real content that should survive.</p></article>
            <div class="sidebar"><p>sidebar stuff</p></div>
        </body></html>"#;
        let default_result = prune_html(html);
        let profile_result = prune_html_with_profile(html, CleaningProfile::Default);
        assert_eq!(default_result, profile_result);
    }

    #[test]
    fn profile_threshold_values() {
        assert!((CleaningProfile::Default.threshold() - 0.48).abs() < f64::EPSILON);
        assert!((CleaningProfile::Minimal.threshold() - 0.20).abs() < f64::EPSILON);
        assert!((CleaningProfile::Aggressive.threshold() - 0.65).abs() < f64::EPSILON);
        assert!((CleaningProfile::ReadingMode.threshold() - 0.45).abs() < f64::EPSILON);
    }

    #[test]
    fn select_profile_no_hint_small_content() {
        assert_eq!(select_profile(None, 1000), CleaningProfile::Default);
    }

    #[test]
    fn select_profile_large_content_always_aggressive() {
        assert_eq!(select_profile(None, 60_000), CleaningProfile::Aggressive);
        assert_eq!(
            select_profile(Some("extract data"), 60_000),
            CleaningProfile::Aggressive
        );
        assert_eq!(
            select_profile(Some("fill form"), 60_000),
            CleaningProfile::Aggressive
        );
    }

    #[test]
    fn select_profile_reading_keywords() {
        assert_eq!(
            select_profile(Some("extract titles"), 1000),
            CleaningProfile::ReadingMode
        );
        assert_eq!(
            select_profile(Some("scrape all data"), 1000),
            CleaningProfile::ReadingMode
        );
        assert_eq!(
            select_profile(Some("read the article"), 1000),
            CleaningProfile::ReadingMode
        );
        assert_eq!(
            select_profile(Some("get data from page"), 1000),
            CleaningProfile::ReadingMode
        );
    }

    #[test]
    fn select_profile_interaction_keywords() {
        assert_eq!(
            select_profile(Some("fill in the login form"), 1000),
            CleaningProfile::Minimal
        );
        assert_eq!(
            select_profile(Some("click the submit button"), 1000),
            CleaningProfile::Minimal
        );
        assert_eq!(
            select_profile(Some("interact with the page"), 1000),
            CleaningProfile::Minimal
        );
    }

    #[test]
    fn select_profile_unknown_hint_returns_default() {
        assert_eq!(
            select_profile(Some("navigate to page"), 1000),
            CleaningProfile::Default
        );
    }

    #[test]
    fn aggressive_produces_smaller_output() {
        let html = "<html><body>
            <article><p>This is a substantial paragraph of real content that should survive most profiles.</p></article>
            <div><p>Some extra content here with moderate text density value.</p></div>
            <section><p>Another section with enough text to potentially survive default.</p></section>
        </body></html>";
        let default_out = prune_html_with_profile(html, CleaningProfile::Default);
        let aggressive_out = prune_html_with_profile(html, CleaningProfile::Aggressive);
        assert!(
            aggressive_out.len() <= default_out.len(),
            "aggressive ({}) should produce output no larger than default ({})",
            aggressive_out.len(),
            default_out.len()
        );
    }

    #[test]
    fn minimal_preserves_more_than_default() {
        let html = "<html><body>
            <div><span>x</span></div>
            <article><p>Content that default keeps.</p></article>
        </body></html>";
        let default_out = prune_html_with_profile(html, CleaningProfile::Default);
        let minimal_out = prune_html_with_profile(html, CleaningProfile::Minimal);
        assert!(
            minimal_out.len() >= default_out.len(),
            "minimal ({}) should preserve at least as much as default ({})",
            minimal_out.len(),
            default_out.len()
        );
    }

    #[test]
    fn reading_mode_boosts_article_content() {
        let html = "<html><body><article><p>Article content here with enough text to score well.</p></article></body></html>";
        let default_score = first_body_child_score_with_profile(html, CleaningProfile::Default);
        let reading_score = first_body_child_score_with_profile(html, CleaningProfile::ReadingMode);
        assert!(
            reading_score >= default_score,
            "ReadingMode score ({reading_score}) should be >= Default ({default_score}) for article"
        );
    }

    #[test]
    fn tag_weight_multiplier_reading_mode() {
        let profile = CleaningProfile::ReadingMode;
        assert!((profile.tag_weight_multiplier("article") - 2.0).abs() < f64::EPSILON);
        assert!((profile.tag_weight_multiplier("p") - 1.5).abs() < f64::EPSILON);
        assert!((profile.tag_weight_multiplier("nav") - 0.1).abs() < f64::EPSILON);
        assert!((profile.tag_weight_multiplier("div") - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn tag_weight_multiplier_minimal_mode() {
        let profile = CleaningProfile::Minimal;
        assert!((profile.tag_weight_multiplier("form") - 2.0).abs() < f64::EPSILON);
        assert!((profile.tag_weight_multiplier("button") - 2.0).abs() < f64::EPSILON);
        assert!((profile.tag_weight_multiplier("div") - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn tag_weight_multiplier_default_always_one() {
        let profile = CleaningProfile::Default;
        assert!((profile.tag_weight_multiplier("article") - 1.0).abs() < f64::EPSILON);
        assert!((profile.tag_weight_multiplier("nav") - 1.0).abs() < f64::EPSILON);
        assert!((profile.tag_weight_multiplier("form") - 1.0).abs() < f64::EPSILON);
    }
}
