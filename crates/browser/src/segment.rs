//! Splits cleaned page HTML into record-level blocks for relevance scoring.
//!
//! Splitting the converted markdown on blank lines cuts records apart (a Hacker News row's
//! rank, title and metadata land in separate paragraphs), so the scorer would judge fragments.
//! Segmenting the DOM instead keeps each row, list item, paragraph or comment whole.

use scraper::{node::Node, ElementRef, Html};

/// Blocks whose text is longer than this are split along their child elements.
pub const DEFAULT_MAX_BLOCK_CHARS: usize = 1_200;

/// Heading level assigned to table headers, so they outrank every real heading.
const THEAD_LEVEL: u8 = 0;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    /// Outer HTML of the block, ready for markdown conversion.
    pub html: String,
    /// `Some(level)` for headings (`h1`..`h6` = 1..6) and table headers (0): context that is
    /// kept only when the content under it is kept, rather than scored on its own.
    pub heading_level: Option<u8>,
}

impl Block {
    #[must_use]
    pub fn is_heading(&self) -> bool {
        self.heading_level.is_some()
    }
}

/// Split `html` into blocks no larger than roughly `max_block_chars` of text, descending into
/// containers only while they are too big. Blocks with no text are dropped.
#[must_use]
pub fn segment_html(html: &str, max_block_chars: usize) -> Vec<Block> {
    let doc = Html::parse_document(html);
    let root = doc
        .select(&scraper::Selector::parse("body").expect("static selector"))
        .next()
        .unwrap_or_else(|| doc.root_element());
    let mut blocks = Vec::new();
    walk(root, max_block_chars, &mut blocks);
    blocks
}

/// Decide which blocks survive. `scores[i]` is the relevance of `blocks[i]` (ignored for
/// headings); a heading is kept when anything under it, down to the next heading of equal or
/// higher rank, is kept.
#[must_use]
pub fn keep_mask(blocks: &[Block], scores: &[f64], threshold: f64) -> Vec<bool> {
    let mut keep: Vec<bool> = blocks
        .iter()
        .zip(scores)
        .map(|(block, score)| !block.is_heading() && *score >= threshold)
        .collect();
    keep.resize(blocks.len(), false);

    for (i, block) in blocks.iter().enumerate() {
        let Some(level) = block.heading_level else {
            continue;
        };
        for j in i + 1..blocks.len() {
            if blocks[j].heading_level.is_some_and(|l| l <= level) {
                break;
            }
            if keep[j] && !blocks[j].is_heading() {
                keep[i] = true;
                break;
            }
        }
    }
    keep
}

fn walk(container: ElementRef<'_>, max_chars: usize, out: &mut Vec<Block>) {
    let mut loose = String::new();
    for child in container.children() {
        match child.value() {
            Node::Text(text) => loose.push_str(&escape_text(text)),
            Node::Element(_) => {
                let Some(el) = ElementRef::wrap(child) else {
                    continue;
                };
                let tag = el.value().name();
                if matches!(tag, "script" | "style" | "noscript" | "template") {
                    continue;
                }
                if is_inline(tag) {
                    loose.push_str(&el.html());
                    continue;
                }
                flush(&mut loose, out);
                push_element(el, max_chars, out);
            }
            _ => {}
        }
    }
    flush(&mut loose, out);
}

fn push_element(el: ElementRef<'_>, max_chars: usize, out: &mut Vec<Block>) {
    let chars = text_len(el);
    if chars == 0 {
        return;
    }
    let tag = el.value().name();
    let level = heading_level(tag);
    let has_block_child = el
        .children()
        .filter_map(ElementRef::wrap)
        .any(|c| !is_inline(c.value().name()));
    if level.is_none() && chars > max_chars && has_block_child {
        walk(el, max_chars, out);
        return;
    }
    out.push(Block {
        html: el.html(),
        heading_level: level,
    });
}

fn flush(loose: &mut String, out: &mut Vec<Block>) {
    if !loose.trim().is_empty() && loose.contains(|c: char| c.is_alphanumeric()) {
        out.push(Block {
            html: format!("<p>{}</p>", loose.trim()),
            heading_level: None,
        });
    }
    loose.clear();
}

fn heading_level(tag: &str) -> Option<u8> {
    match tag {
        "h1" => Some(1),
        "h2" => Some(2),
        "h3" => Some(3),
        "h4" => Some(4),
        "h5" => Some(5),
        "h6" => Some(6),
        "thead" => Some(THEAD_LEVEL),
        _ => None,
    }
}

fn is_inline(tag: &str) -> bool {
    matches!(
        tag,
        "a" | "abbr"
            | "b"
            | "bdi"
            | "bdo"
            | "br"
            | "cite"
            | "code"
            | "data"
            | "del"
            | "dfn"
            | "em"
            | "font"
            | "i"
            | "img"
            | "ins"
            | "kbd"
            | "mark"
            | "q"
            | "s"
            | "samp"
            | "small"
            | "span"
            | "strong"
            | "sub"
            | "sup"
            | "time"
            | "u"
            | "var"
            | "wbr"
    )
}

/// Characters of visible text under `el`, ignoring script/style payloads.
fn text_len(el: ElementRef<'_>) -> usize {
    el.descendants()
        .filter_map(|node| match node.value() {
            Node::Text(text) => {
                let hidden = node.parent().and_then(ElementRef::wrap).is_some_and(|p| {
                    matches!(
                        p.value().name(),
                        "script" | "style" | "noscript" | "template"
                    )
                });
                (!hidden).then(|| text.trim().chars().count())
            }
            _ => None,
        })
        .sum()
}

fn escape_text(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(blocks: &[Block]) -> Vec<String> {
        blocks
            .iter()
            .map(|b| {
                Html::parse_fragment(&b.html)
                    .root_element()
                    .text()
                    .collect::<String>()
                    .trim()
                    .to_string()
            })
            .collect()
    }

    #[test]
    fn keeps_a_table_row_whole() {
        let html = "<table><tr><td>1.</td><td><a href='/x'>A title</a></td><td>(site)</td></tr>\
                    <tr><td>42 points by bob</td></tr></table>";
        let blocks = segment_html(html, 1_200);
        let t = texts(&blocks);
        assert_eq!(blocks.len(), 1, "small table stays one block: {t:?}");
    }

    #[test]
    fn splits_oversized_containers_into_rows() {
        let row = |n: usize| format!("<tr><td>row {n} {}</td></tr>", "word ".repeat(30));
        let rows: String = (0..20).map(row).collect();
        let blocks = segment_html(&format!("<table><tbody>{rows}</tbody></table>"), 600);
        assert_eq!(blocks.len(), 20);
        assert!(texts(&blocks)[7].starts_with("row 7"));
    }

    #[test]
    fn marks_headings_and_drops_empty_blocks() {
        let blocks = segment_html(
            "<h2>Install</h2><div></div><p>Run it.</p><script>x</script>",
            100,
        );
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].heading_level, Some(2));
        assert_eq!(blocks[1].heading_level, None);
    }

    #[test]
    fn loose_text_and_inline_runs_become_one_block() {
        let blocks = segment_html("<div>Hello <b>big</b> world<p>Next</p></div>", 5);
        assert_eq!(texts(&blocks), vec!["Hello big world", "Next"]);
    }

    #[test]
    fn script_payloads_do_not_count_as_text() {
        let json = "x".repeat(5_000);
        let blocks = segment_html(
            &format!("<div><script>{json}</script></div><p>Real.</p>"),
            100,
        );
        assert_eq!(texts(&blocks), vec!["Real."]);
    }

    #[test]
    fn heading_is_kept_only_when_its_section_has_kept_content() {
        let blocks = segment_html(
            "<h1>Doc</h1><h2>A</h2><p>a1</p><h2>B</h2><p>b1</p><p>b2</p>",
            100,
        );
        // h1, h2 A, a1, h2 B, b1, b2
        let scores = [0.0, 0.0, 0.1, 0.0, 0.2, 0.9];
        let keep = keep_mask(&blocks, &scores, 0.35);
        assert_eq!(keep, vec![true, false, false, true, false, true]);
    }

    #[test]
    fn table_head_survives_with_any_kept_row() {
        let blocks = segment_html(
            "<table><thead><tr><th>Name</th></tr></thead><tbody><tr><td>x</td></tr><tr><td>y</td></tr></tbody></table>",
            4,
        );
        let scores = vec![0.0; blocks.len()];
        assert!(keep_mask(&blocks, &scores, 0.35).iter().all(|k| !k));
        let mut scores = scores;
        *scores.last_mut().unwrap() = 0.9;
        let keep = keep_mask(&blocks, &scores, 0.35);
        assert!(keep[0] && keep[keep.len() - 1]);
    }
}
