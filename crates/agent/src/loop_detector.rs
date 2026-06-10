use std::collections::VecDeque;

use serde_json::Value;

use crate::page_fingerprint::PageFingerprint;

const DEFAULT_WINDOW: usize = 20;
const DEFAULT_NUDGE_THRESHOLD: usize = 5;

#[derive(Debug, Clone)]
pub enum LoopNudge {
    Soft(String),
    Medium(String),
    Strong(String),
    Stagnation(String),
}

impl LoopNudge {
    #[must_use]
    pub fn message(&self) -> &str {
        match self {
            Self::Soft(message)
            | Self::Medium(message)
            | Self::Strong(message)
            | Self::Stagnation(message) => message,
        }
    }
}

#[derive(Debug, Clone)]
pub struct LoopDetector {
    action_window: VecDeque<u64>,
    page_fingerprints: VecDeque<String>,
    window_size: usize,
    nudge_threshold: usize,
}

impl Default for LoopDetector {
    fn default() -> Self {
        Self::new(DEFAULT_WINDOW, DEFAULT_NUDGE_THRESHOLD)
    }
}

impl LoopDetector {
    #[must_use]
    pub fn new(window_size: usize, nudge_threshold: usize) -> Self {
        Self {
            action_window: VecDeque::new(),
            page_fingerprints: VecDeque::new(),
            window_size: window_size.max(1),
            nudge_threshold: nudge_threshold.max(1),
        }
    }

    /// Record a tool action. Tool name + normalized input is hashed.
    pub fn record_action(&mut self, tool_name: &str, input: &Value) {
        let hash = hash_action(tool_name, input);
        self.action_window.push_back(hash);
        while self.action_window.len() > self.window_size {
            let _ = self.action_window.pop_front();
        }
    }

    /// Record a page state fingerprint.
    pub fn record_page_state(&mut self, fingerprint: &PageFingerprint) {
        let key = format!(
            "{}|{}|{}",
            fingerprint.url, fingerprint.element_count, fingerprint.text_hash
        );
        self.page_fingerprints.push_back(key);
        while self.page_fingerprints.len() > self.window_size {
            let _ = self.page_fingerprints.pop_front();
        }
    }

    /// Check for repetition patterns. Returns a nudge if a loop is detected.
    #[must_use]
    pub fn detect_loop(&self) -> Option<LoopNudge> {
        if self.action_window.len() >= self.nudge_threshold {
            let last = *self.action_window.back()?;
            let repeat_count = self
                .action_window
                .iter()
                .rev()
                .take(self.window_size)
                .take_while(|&&hash| hash == last)
                .count();

            if repeat_count >= 12 {
                return Some(LoopNudge::Strong(format!(
                    "You have repeated the same action {repeat_count} times. This approach is not working. You MUST try a completely different strategy."
                )));
            }
            if repeat_count >= 8 {
                return Some(LoopNudge::Medium(format!(
                    "Your actions seem repetitive ({repeat_count} identical actions). Consider a significantly different approach."
                )));
            }
            if repeat_count >= self.nudge_threshold {
                return Some(LoopNudge::Soft(
                    "Consider a different approach — you may be repeating actions that haven't worked."
                        .to_string(),
                ));
            }
        }

        if self.page_fingerprints.len() >= self.nudge_threshold {
            let last = self.page_fingerprints.back()?;
            let stagnant_count = self
                .page_fingerprints
                .iter()
                .rev()
                .take(self.nudge_threshold)
                .filter(|fingerprint| *fingerprint == last)
                .count();
            if stagnant_count >= self.nudge_threshold {
                return Some(LoopNudge::Stagnation(
                    "The page state has not changed for several steps. You may be stuck. Try a different action or navigate elsewhere."
                        .to_string(),
                ));
            }
        }

        None
    }
}

/// Normalize action to a stable hash.
fn hash_action(tool_name: &str, input: &Value) -> u64 {
    let key = match tool_name {
        "click" => {
            let selector = input.get("selector").and_then(Value::as_str).unwrap_or("");
            format!("click:{selector}")
        }
        "fill_form" => {
            let fields = input
                .get("fields")
                .map(|value| serde_json::to_string(value).unwrap_or_default())
                .unwrap_or_default()
                .to_lowercase();
            format!("fill_form:{fields}")
        }
        "navigate" => {
            let url = input.get("url").and_then(Value::as_str).unwrap_or("");
            format!("navigate:{url}")
        }
        "scroll" => {
            let direction = input.get("direction").and_then(Value::as_str).unwrap_or("");
            format!("scroll:{direction}")
        }
        _ => {
            let canonical = serde_json::to_string(input).unwrap_or_default();
            format!("{tool_name}:{canonical}")
        }
    };
    fnv1a_hash(&key)
}

fn fnv1a_hash(input: &str) -> u64 {
    const FNV_OFFSET: u64 = 14_695_981_039_346_656_037;
    const FNV_PRIME: u64 = 1_099_511_628_211;
    let mut hash = FNV_OFFSET;
    for byte in input.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn make_fp(url: &str) -> PageFingerprint {
        PageFingerprint {
            url: url.to_string(),
            element_count: 5,
            text_hash: 12_345,
        }
    }

    #[test]
    fn soft_nudge_at_threshold() {
        let mut detector = LoopDetector::new(20, 5);
        let input = json!({"selector": "@e3"});
        for _ in 0..5 {
            detector.record_action("click", &input);
        }
        let nudge = detector.detect_loop().expect("expected soft nudge");
        assert!(matches!(nudge, LoopNudge::Soft(_)));
        assert!(nudge.message().contains("different approach"));
    }

    #[test]
    fn no_nudge_below_threshold() {
        let mut detector = LoopDetector::new(20, 5);
        let input = json!({"selector": "@e3"});
        for _ in 0..4 {
            detector.record_action("click", &input);
        }
        assert!(detector.detect_loop().is_none());
    }

    #[test]
    fn different_actions_no_false_positive() {
        let mut detector = LoopDetector::new(20, 5);
        for i in 0..10 {
            detector.record_action("click", &json!({"selector": format!("@e{i}")}));
        }
        assert!(detector.detect_loop().is_none());
    }

    #[test]
    fn navigate_different_urls_no_false_positive() {
        let mut detector = LoopDetector::new(20, 5);
        for i in 1..=6 {
            detector.record_action(
                "navigate",
                &json!({"url": format!("https://example.com/page{i}")}),
            );
        }
        assert!(detector.detect_loop().is_none());
    }

    #[test]
    fn stagnation_after_five_identical_fingerprints() {
        let mut detector = LoopDetector::new(20, 5);
        let fingerprint = make_fp("https://example.com");
        for _ in 0..5 {
            detector.record_page_state(&fingerprint);
        }
        let nudge = detector.detect_loop().expect("expected stagnation nudge");
        assert!(matches!(nudge, LoopNudge::Stagnation(_)));
    }

    #[test]
    fn strong_nudge_at_twelve_repeats() {
        let mut detector = LoopDetector::new(20, 5);
        let input = json!({"selector": "@e3"});
        for _ in 0..12 {
            detector.record_action("click", &input);
        }
        let nudge = detector.detect_loop().expect("expected strong nudge");
        assert!(matches!(nudge, LoopNudge::Strong(_)));
    }
}
