//! Goal-aware pruning: drop page blocks a relevance model judges unrelated to the caller's
//! `focus`, leaving a tiny `[N blocks pruned]` marker where they were.
//!
//! The scorer is the `TypeSafe` "Jev" model, which answers a yes/no question about each block
//! with a probability. Scoring is best-effort: every failure path returns an error and the
//! caller keeps the heuristically pruned page, so this layer can only ever remove less.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use browser::markdown::html_to_markdown;
use browser::segment::{keep_mask, segment_html, Block, DEFAULT_MAX_BLOCK_CHARS};
use serde_json::{json, Value};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

/// Blocks scoring at or above this are kept. 0.2–0.35 held recall in testing; 0.5+ lost it.
pub const KEEP_THRESHOLD: f64 = 0.35;
/// Pages with less text than this are returned as-is: nothing worth a network round trip.
pub const MIN_PAGE_CHARS: usize = 1_500;
const ENDPOINT: &str = "https://api.typesafe.ai/v1/systemone";
const MODEL: &str = "jev-latest";
/// Blocks per request. Scores shift a little with chunk size, so it is fixed.
const CHUNK_BLOCKS: usize = 40;
/// Characters of each block shown to the scorer.
const SCORED_CHARS: usize = 500;
const MAX_IN_FLIGHT: usize = 8;
/// Most blocks scored for one page. Bounds API spend and latency per navigation; a page with more
/// candidate blocks is left to the heuristic pruner instead of being scored in part.
const MAX_SCORED_BLOCKS: usize = CHUNK_BLOCKS * 10;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const RETRY_DELAY: Duration = Duration::from_millis(400);
pub const API_KEY_ENV: &str = "TYPESAFE_API_KEY";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelevanceError(pub String);

impl std::fmt::Display for RelevanceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for RelevanceError {}

pub type ScoreFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Vec<f64>, RelevanceError>> + Send + 'a>>;

/// Rates how relevant each text block is to a goal, as probabilities in `[0, 1]`.
pub trait RelevanceScorer: Send + Sync {
    /// One score per entry of `blocks`, in order.
    fn score<'a>(&'a self, focus: &'a [String], blocks: &'a [String]) -> ScoreFuture<'a>;
}

/// Result of pruning a page to a focus.
#[derive(Debug, Clone, PartialEq)]
pub struct FocusedPage {
    pub markdown: String,
    pub kept_blocks: usize,
    pub pruned_blocks: usize,
}

/// Prune `cleaned_html` down to the blocks relevant to `focus`.
///
/// `max_chars` bounds the markdown: when more blocks pass the threshold than fit, the
/// lowest-scoring ones are dropped rather than the end of the page being cut off.
///
/// Returns `Ok(None)` when pruning would not help or would leave nothing (page too small, no
/// text blocks, or every block scores below the threshold); the caller keeps its own output.
pub async fn prune_to_focus(
    scorer: &dyn RelevanceScorer,
    focus: &[String],
    cleaned_html: &str,
    max_chars: usize,
) -> Result<Option<FocusedPage>, RelevanceError> {
    let blocks = segment_html(cleaned_html, DEFAULT_MAX_BLOCK_CHARS);
    let mut md: Vec<String> = blocks
        .iter()
        .map(|b| html_to_markdown(&b.html).trim().to_string())
        .collect();

    let total_chars: usize = md.iter().map(|m| m.chars().count()).sum();
    if total_chars < MIN_PAGE_CHARS {
        return Ok(None);
    }

    // Headings are context for the content below them, not scorable themselves.
    let scorable: Vec<usize> = (0..blocks.len())
        .filter(|&i| !blocks[i].is_heading() && !md[i].is_empty())
        .collect();
    if scorable.len() < 2 {
        return Ok(None);
    }
    let texts: Vec<String> = scorable
        .iter()
        .map(|&i| clip_chars(&md[i], SCORED_CHARS))
        .collect();
    let probs = scorer.score(focus, &texts).await?;
    if probs.len() != scorable.len() {
        return Err(RelevanceError(format!(
            "scorer returned {} scores for {} blocks",
            probs.len(),
            scorable.len()
        )));
    }

    let mut per_block = vec![0.0; blocks.len()];
    for (&i, p) in scorable.iter().zip(&probs) {
        per_block[i] = *p;
    }
    let mut keep = keep_mask(&blocks, &per_block, KEEP_THRESHOLD);
    if !scorable.iter().any(|&i| keep[i]) {
        return Ok(None);
    }
    if kept_chars(&md, &keep) > max_chars {
        fit_to_budget(&blocks, &mut md, &mut per_block, &mut keep, max_chars);
        if !scorable.iter().any(|&i| keep[i]) {
            return Ok(None);
        }
    }
    Ok(Some(assemble(&md, &keep)))
}

fn kept_chars(md: &[String], keep: &[bool]) -> usize {
    md.iter()
        .zip(keep)
        .filter(|(_, &k)| k)
        .map(|(m, _)| m.chars().count() + 2)
        .sum()
}

/// Keep the highest-scoring blocks that fit in `max_chars` (headings and markers included),
/// preserving document order.
fn fit_to_budget(
    blocks: &[Block],
    md: &mut [String],
    per_block: &mut [f64],
    keep: &mut Vec<bool>,
    max_chars: usize,
) {
    let mut ranked: Vec<usize> = (0..blocks.len())
        .filter(|&i| keep[i] && !blocks[i].is_heading())
        .collect();
    ranked.sort_by(|&a, &b| per_block[b].total_cmp(&per_block[a]).then(a.cmp(&b)));

    let total = max_chars.saturating_sub(max_chars / 20);
    let mut budget = total;
    let mut chosen = vec![false; blocks.len()];
    for &i in &ranked {
        let cost = md[i].chars().count() + 2;
        if cost <= budget {
            budget -= cost;
            chosen[i] = true;
        }
    }
    // A single relevant block can be longer than the whole budget (a long paragraph or `<pre>`).
    // Shorten the best one rather than dropping everything and returning only markers.
    if let (false, Some(&top)) = (chosen.iter().any(|&c| c), ranked.first()) {
        let room = total.saturating_sub(2);
        if room > 0 {
            let head = clip_chars(&md[top], room.saturating_sub(1));
            md[top] = format!("{head}…");
            chosen[top] = true;
        }
    }
    for (score, &picked) in per_block.iter_mut().zip(&chosen) {
        if !picked {
            *score = 0.0;
        }
    }
    *keep = keep_mask(blocks, per_block, KEEP_THRESHOLD);
}

fn assemble(markdown: &[String], keep: &[bool]) -> FocusedPage {
    let mut parts: Vec<String> = Vec::new();
    let (mut kept, mut pruned, mut run) = (0, 0, 0);
    for (md, &keep) in markdown.iter().zip(keep) {
        if md.is_empty() {
            continue;
        }
        if keep {
            if run > 0 {
                parts.push(placeholder(run));
                run = 0;
            }
            parts.push(md.clone());
            kept += 1;
        } else {
            run += 1;
            pruned += 1;
        }
    }
    if run > 0 {
        parts.push(placeholder(run));
    }
    FocusedPage {
        markdown: parts.join("\n\n"),
        kept_blocks: kept,
        pruned_blocks: pruned,
    }
}

fn placeholder(blocks: usize) -> String {
    format!("[{blocks} blocks pruned]")
}

fn clip_chars(text: &str, max: usize) -> String {
    text.char_indices()
        .nth(max)
        .map_or_else(|| text.to_string(), |(idx, _)| text[..idx].to_string())
}

/// `focus` as one goal string: the text itself, or an any-of list when there are several.
fn goal_text(focus: &[String]) -> String {
    match focus {
        [one] => one.clone(),
        many => {
            let listed: Vec<String> = many
                .iter()
                .enumerate()
                .map(|(i, f)| format!("({}) {f}", i + 1))
                .collect();
            format!("any of: {}", listed.join(" "))
        }
    }
}

/// Request body scoring `chunk`, whose first block is global block number `offset`.
fn build_request(focus: &[String], offset: usize, chunk: &[String]) -> Value {
    let mut blocks = Vec::with_capacity(chunk.len());
    let mut questions = serde_json::Map::new();
    for (j, text) in chunk.iter().enumerate() {
        let id = format!("b{}", offset + j);
        questions.insert(
            id.clone(),
            json!({
                "type": "noul",
                "instructions": format!(
                    "Is block {id} relevant to the goal? It must contain information needed to accomplish the goal."
                ),
            }),
        );
        blocks.push(json!({ "id": id, "text": text }));
    }
    json!({
        "model": MODEL,
        "state": { "goal": goal_text(focus), "blocks": blocks },
        "questions": questions,
    })
}

fn parse_response(
    response: &Value,
    offset: usize,
    count: usize,
) -> Result<Vec<f64>, RelevanceError> {
    let answers = response
        .get("answers")
        .and_then(Value::as_object)
        .ok_or_else(|| RelevanceError("response has no `answers`".to_string()))?;
    (0..count)
        .map(|j| {
            let id = format!("b{}", offset + j);
            answers
                .get(&id)
                .and_then(|a| a.get("noul"))
                .and_then(Value::as_f64)
                .map(|p| p.clamp(0.0, 1.0))
                .ok_or_else(|| RelevanceError(format!("no score for block {id}")))
        })
        .collect()
}

/// Scores blocks with the `TypeSafe` Jev model.
#[derive(Clone)]
pub struct JevScorer {
    client: reqwest::Client,
    api_key: Arc<str>,
    endpoint: Arc<str>,
}

impl JevScorer {
    /// The API key to use: `configured` (from settings) first, then `TYPESAFE_API_KEY`.
    /// Blank values count as unset.
    #[must_use]
    pub fn resolve_api_key(configured: Option<&str>) -> Option<String> {
        let non_blank = |key: &str| {
            let key = key.trim();
            (!key.is_empty()).then(|| key.to_string())
        };
        configured.and_then(non_blank).or_else(|| {
            std::env::var(API_KEY_ENV)
                .ok()
                .as_deref()
                .and_then(non_blank)
        })
    }

    /// A scorer authenticating with `key`, or `None` when it is blank or the HTTP client
    /// cannot be built.
    #[must_use]
    pub fn new(key: &str) -> Option<Self> {
        let key = key.trim();
        if key.is_empty() {
            return None;
        }
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .ok()?;
        Some(Self {
            client,
            api_key: key.into(),
            endpoint: ENDPOINT.into(),
        })
    }

    async fn score_chunk(
        &self,
        focus: &[String],
        offset: usize,
        chunk: &[String],
    ) -> Result<Vec<f64>, RelevanceError> {
        let body = build_request(focus, offset, chunk);
        let mut last = RelevanceError("no attempt made".to_string());
        for attempt in 0..2 {
            if attempt > 0 {
                tokio::time::sleep(RETRY_DELAY).await;
            }
            match self.post(&body).await {
                Ok(response) => return parse_response(&response, offset, chunk.len()),
                Err(error) => last = error,
            }
        }
        Err(last)
    }

    async fn post(&self, body: &Value) -> Result<Value, RelevanceError> {
        let response = self
            .client
            .post(&*self.endpoint)
            .bearer_auth(&*self.api_key)
            .json(body)
            .send()
            .await
            .map_err(|e| RelevanceError(format!("request failed: {}", e.without_url())))?;
        let status = response.status();
        if !status.is_success() {
            return Err(RelevanceError(format!("scoring API returned {status}")));
        }
        response
            .json()
            .await
            .map_err(|e| RelevanceError(format!("bad response body: {}", e.without_url())))
    }
}

impl RelevanceScorer for JevScorer {
    fn score<'a>(&'a self, focus: &'a [String], blocks: &'a [String]) -> ScoreFuture<'a> {
        Box::pin(async move {
            if blocks.len() > MAX_SCORED_BLOCKS {
                return Err(RelevanceError(format!(
                    "page has {} candidate blocks; scoring is capped at {MAX_SCORED_BLOCKS}",
                    blocks.len()
                )));
            }
            let gate = Arc::new(Semaphore::new(MAX_IN_FLIGHT));
            let focus: Arc<[String]> = focus.into();
            let mut tasks = JoinSet::new();
            for (n, chunk) in blocks.chunks(CHUNK_BLOCKS).enumerate() {
                let (this, gate, focus, chunk) =
                    (self.clone(), gate.clone(), focus.clone(), chunk.to_vec());
                tasks.spawn(async move {
                    let _permit = gate
                        .acquire_owned()
                        .await
                        .map_err(|e| RelevanceError(e.to_string()))?;
                    let offset = n * CHUNK_BLOCKS;
                    this.score_chunk(&focus, offset, &chunk)
                        .await
                        .map(|scores| (n, scores))
                });
            }
            let mut by_chunk = vec![Vec::new(); blocks.len().div_ceil(CHUNK_BLOCKS)];
            while let Some(joined) = tasks.join_next().await {
                let (n, scores) = joined.map_err(|e| RelevanceError(e.to_string()))??;
                by_chunk[n] = scores;
            }
            Ok(by_chunk.concat())
        })
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Write;

    use super::*;

    /// Scores a block 1.0 when it contains the needle, else 0.0.
    struct Needle(&'static str);

    impl RelevanceScorer for Needle {
        fn score<'a>(&'a self, _focus: &'a [String], blocks: &'a [String]) -> ScoreFuture<'a> {
            Box::pin(async move {
                Ok(blocks
                    .iter()
                    .map(|b| if b.contains(self.0) { 1.0 } else { 0.0 })
                    .collect())
            })
        }
    }

    struct Failing;

    impl RelevanceScorer for Failing {
        fn score<'a>(&'a self, _: &'a [String], _: &'a [String]) -> ScoreFuture<'a> {
            Box::pin(async { Err(RelevanceError("boom".to_string())) })
        }
    }

    fn focus() -> Vec<String> {
        vec!["pricing".to_string()]
    }

    fn page(paragraphs: usize) -> String {
        let mut html = String::from("<h2>Plans</h2>");
        for n in 0..paragraphs {
            let topic = if n % 5 == 0 {
                "NEEDLE pricing"
            } else {
                "unrelated chatter"
            };
            let _ = write!(html, "<p>{topic} number {n}. {}</p>", "filler ".repeat(30));
        }
        html
    }

    #[tokio::test]
    async fn keeps_relevant_blocks_and_collapses_runs_into_markers() {
        let out = prune_to_focus(&Needle("NEEDLE"), &focus(), &page(11), 100_000)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            out.kept_blocks,
            3 + 1,
            "3 paragraphs plus the heading above them"
        );
        assert_eq!(out.pruned_blocks, 8);
        assert!(out.markdown.contains("number 0."));
        assert!(!out.markdown.contains("number 1."));
        assert!(out.markdown.contains("[4 blocks pruned]"));
        assert!(out.markdown.starts_with("## Plans"));
    }

    /// Scores by the number after "number ", so higher-numbered blocks rank higher.
    struct ByNumber;

    impl RelevanceScorer for ByNumber {
        fn score<'a>(&'a self, _: &'a [String], blocks: &'a [String]) -> ScoreFuture<'a> {
            Box::pin(async move {
                Ok(blocks
                    .iter()
                    .map(|b| {
                        let n: f64 = b
                            .split("number ")
                            .nth(1)
                            .and_then(|r| r.split('.').next())
                            .and_then(|n| n.parse().ok())
                            .unwrap_or(0.0);
                        0.5 + n / 100.0
                    })
                    .collect())
            })
        }
    }

    #[tokio::test]
    async fn over_budget_pages_keep_the_best_blocks_not_the_first() {
        // Every block passes the threshold, but only ~3 fit; the highest-numbered win.
        let out = prune_to_focus(&ByNumber, &focus(), &page(20), 800)
            .await
            .unwrap()
            .unwrap();
        assert!(
            out.markdown.chars().count() <= 800,
            "{}",
            out.markdown.len()
        );
        assert!(out.markdown.contains("number 19."));
        assert!(!out.markdown.contains("number 0."));
        assert!(out.markdown.contains("blocks pruned]"));
    }

    #[tokio::test]
    async fn a_relevant_block_longer_than_the_budget_is_shortened_not_dropped() {
        let html = format!(
            "<h2>Spec</h2><p>NEEDLE {}</p><p>unrelated one {}</p><p>unrelated two {}</p>",
            "long ".repeat(800),
            "filler ".repeat(40),
            "filler ".repeat(40),
        );
        let out = prune_to_focus(&Needle("NEEDLE"), &focus(), &html, 1_000)
            .await
            .unwrap()
            .expect("the relevant block must survive");
        assert!(out.markdown.contains("NEEDLE"), "{}", out.markdown);
        assert!(out.markdown.contains('…'), "{}", out.markdown);
        assert!(out.markdown.chars().count() <= 1_000, "{}", out.markdown);
        assert_eq!(out.kept_blocks, 2, "the heading plus the shortened block");
    }

    #[tokio::test]
    async fn jev_scorer_refuses_pages_over_the_block_cap_before_any_request() {
        let scorer = JevScorer::new("tsk-test").expect("scorer");
        let blocks = vec!["block".to_string(); MAX_SCORED_BLOCKS + 1];
        let err = scorer.score(&focus(), &blocks).await.unwrap_err();
        assert!(err.0.contains("capped"), "{err}");
    }

    #[test]
    fn configured_key_wins_and_is_trimmed() {
        assert_eq!(
            JevScorer::resolve_api_key(Some("  tsk-settings ")).as_deref(),
            Some("tsk-settings")
        );
    }

    #[test]
    fn placeholder_is_tiny() {
        assert_eq!(placeholder(212), "[212 blocks pruned]");
    }

    #[tokio::test]
    async fn returns_none_when_nothing_matches() {
        let out = prune_to_focus(&Needle("ABSENT"), &focus(), &page(11), 100_000).await;
        assert_eq!(out, Ok(None));
    }

    #[tokio::test]
    async fn small_pages_skip_scoring() {
        let out = prune_to_focus(&Failing, &focus(), "<p>tiny page</p>", 100_000).await;
        assert_eq!(out, Ok(None), "scorer must not be called for small pages");
    }

    #[tokio::test]
    async fn propagates_scorer_errors() {
        let out = prune_to_focus(&Failing, &focus(), &page(11), 100_000).await;
        assert_eq!(out, Err(RelevanceError("boom".to_string())));
    }

    #[test]
    fn request_labels_blocks_with_global_ids() {
        let body = build_request(&focus(), 40, &["a".to_string(), "b".to_string()]);
        assert_eq!(body["state"]["goal"], "pricing");
        assert_eq!(body["state"]["blocks"][1]["id"], "b41");
        assert_eq!(body["questions"]["b40"]["type"], "noul");
        assert!(body["questions"]["b41"]["instructions"]
            .as_str()
            .unwrap()
            .contains("b41"));
    }

    #[test]
    fn multiple_focus_items_become_an_any_of_goal() {
        let goal = goal_text(&["install".to_string(), "licence".to_string()]);
        assert_eq!(goal, "any of: (1) install (2) licence");
    }

    #[test]
    fn parses_scores_and_rejects_missing_answers() {
        let response = json!({"answers": {"b0": {"noul": 0.9}, "b1": {"noul": 1.4}}});
        assert_eq!(parse_response(&response, 0, 2).unwrap(), vec![0.9, 1.0]);
        assert!(parse_response(&response, 0, 3).is_err());
        assert!(parse_response(&json!({}), 0, 1).is_err());
    }

    #[test]
    fn clip_chars_respects_char_boundaries() {
        assert_eq!(clip_chars("héllo wörld", 4), "héll");
        assert_eq!(clip_chars("short", 10), "short");
    }
}
