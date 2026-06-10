#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Confidence {
    High,
    Medium,
    Low,
}

#[derive(Debug, Clone, Default)]
pub struct ConfidenceTracker {
    pub last: Option<Confidence>,
    pub consecutive_low: u8,
}

impl ConfidenceTracker {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Parse confidence marker from assistant response text.
    /// Looks for pattern: `[confidence: HIGH]`, `[confidence: MEDIUM]`, `[confidence: LOW]`
    #[must_use]
    pub fn parse_from_text(text: &str) -> Option<Confidence> {
        let lower = text.to_lowercase();
        if let Some(start) = lower.find("[confidence:") {
            let rest = &lower[start..];
            if let Some(end) = rest.find(']') {
                let inner = &rest[..=end];
                if inner.contains("high") {
                    return Some(Confidence::High);
                } else if inner.contains("low") {
                    return Some(Confidence::Low);
                } else if inner.contains("medium") {
                    return Some(Confidence::Medium);
                }
            }
        }
        None
    }

    /// Record a new confidence value. Returns true if stagnation alert should
    /// be injected (2+ consecutive LOWs).
    pub fn record(&mut self, confidence: Confidence) -> bool {
        if confidence == Confidence::Low {
            self.consecutive_low += 1;
        } else {
            self.consecutive_low = 0;
        }
        self.last = Some(confidence);
        self.consecutive_low >= 2
    }
}

/// Build the confidence instruction to inject into `DynamicPromptContext`.
#[must_use]
pub fn confidence_instruction() -> String {
    "After each action, rate your confidence in the current approach. \
     Add exactly: [confidence: HIGH], [confidence: MEDIUM], or [confidence: LOW] \
     at the end of your response."
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_high_confidence() {
        assert_eq!(
            ConfidenceTracker::parse_from_text("...done. [confidence: HIGH]"),
            Some(Confidence::High)
        );
    }

    #[test]
    fn parse_low_confidence() {
        assert_eq!(
            ConfidenceTracker::parse_from_text("stuck. [confidence: LOW]"),
            Some(Confidence::Low)
        );
    }

    #[test]
    fn parse_medium_confidence() {
        assert_eq!(
            ConfidenceTracker::parse_from_text("[confidence: MEDIUM]"),
            Some(Confidence::Medium)
        );
    }

    #[test]
    fn parse_none_when_absent() {
        assert_eq!(ConfidenceTracker::parse_from_text("No marker here."), None);
    }

    #[test]
    fn parse_case_insensitive() {
        assert_eq!(
            ConfidenceTracker::parse_from_text("[confidence: high]"),
            Some(Confidence::High)
        );
        assert_eq!(
            ConfidenceTracker::parse_from_text("[Confidence: Low]"),
            Some(Confidence::Low)
        );
    }

    #[test]
    fn stagnation_triggers_at_2_consecutive_lows() {
        let mut tracker = ConfidenceTracker::new();
        assert!(!tracker.record(Confidence::Low)); // 1 LOW — no alert
        assert!(tracker.record(Confidence::Low)); // 2 LOWs — alert!
    }

    #[test]
    fn consecutive_reset_on_non_low() {
        let mut tracker = ConfidenceTracker::new();
        tracker.record(Confidence::Low);
        tracker.record(Confidence::High); // resets consecutive
        assert!(!tracker.record(Confidence::Low)); // back to 1 — no alert
    }

    #[test]
    fn three_consecutive_lows_still_alerts() {
        let mut tracker = ConfidenceTracker::new();
        assert!(!tracker.record(Confidence::Low));
        assert!(tracker.record(Confidence::Low));
        assert!(tracker.record(Confidence::Low)); // 3rd consecutive — still alerts
    }

    #[test]
    fn confidence_instruction_non_empty() {
        let instr = confidence_instruction();
        assert!(instr.contains("[confidence: HIGH]"));
        assert!(instr.contains("[confidence: LOW]"));
    }
}
