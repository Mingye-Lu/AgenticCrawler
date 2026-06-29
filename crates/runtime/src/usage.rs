use crate::session::Session;
pub use acrawl_core::message::TokenUsage;

const DEFAULT_INPUT_COST_PER_MILLION: f64 = 15.0;
const DEFAULT_OUTPUT_COST_PER_MILLION: f64 = 75.0;
const DEFAULT_CACHE_CREATION_COST_PER_MILLION: f64 = 18.75;
const DEFAULT_CACHE_READ_COST_PER_MILLION: f64 = 1.5;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelPricing {
    pub input_cost_per_million: f64,
    pub output_cost_per_million: f64,
    pub cache_creation_cost_per_million: f64,
    pub cache_read_cost_per_million: f64,
}

impl ModelPricing {
    #[must_use]
    pub const fn default_sonnet_tier() -> Self {
        Self {
            input_cost_per_million: DEFAULT_INPUT_COST_PER_MILLION,
            output_cost_per_million: DEFAULT_OUTPUT_COST_PER_MILLION,
            cache_creation_cost_per_million: DEFAULT_CACHE_CREATION_COST_PER_MILLION,
            cache_read_cost_per_million: DEFAULT_CACHE_READ_COST_PER_MILLION,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UsageCostEstimate {
    pub input_cost_usd: f64,
    pub output_cost_usd: f64,
    pub cache_creation_cost_usd: f64,
    pub cache_read_cost_usd: f64,
}

impl UsageCostEstimate {
    #[must_use]
    pub fn total_cost_usd(self) -> f64 {
        self.input_cost_usd
            + self.output_cost_usd
            + self.cache_creation_cost_usd
            + self.cache_read_cost_usd
    }
}

#[must_use]
pub fn pricing_for_model(model: &str) -> Option<ModelPricing> {
    let normalized = model.to_ascii_lowercase();
    if normalized.contains("haiku") {
        return Some(ModelPricing {
            input_cost_per_million: 1.0,
            output_cost_per_million: 5.0,
            cache_creation_cost_per_million: 1.25,
            cache_read_cost_per_million: 0.1,
        });
    }
    if normalized.contains("opus") {
        return Some(ModelPricing {
            input_cost_per_million: 15.0,
            output_cost_per_million: 75.0,
            cache_creation_cost_per_million: 18.75,
            cache_read_cost_per_million: 1.5,
        });
    }
    if normalized.contains("sonnet") {
        return Some(ModelPricing::default_sonnet_tier());
    }
    None
}

#[must_use]
pub fn estimate_cost_usd(usage: TokenUsage) -> UsageCostEstimate {
    estimate_cost_usd_with_pricing(usage, ModelPricing::default_sonnet_tier())
}

#[must_use]
pub fn estimate_cost_usd_with_pricing(
    usage: TokenUsage,
    pricing: ModelPricing,
) -> UsageCostEstimate {
    UsageCostEstimate {
        input_cost_usd: cost_for_tokens(usage.input_tokens, pricing.input_cost_per_million),
        output_cost_usd: cost_for_tokens(usage.output_tokens, pricing.output_cost_per_million),
        cache_creation_cost_usd: cost_for_tokens(
            usage.cache_creation_input_tokens,
            pricing.cache_creation_cost_per_million,
        ),
        cache_read_cost_usd: cost_for_tokens(
            usage.cache_read_input_tokens,
            pricing.cache_read_cost_per_million,
        ),
    }
}

#[must_use]
pub fn summary_lines(usage: TokenUsage, label: &str) -> Vec<String> {
    summary_lines_for_model(usage, label, None)
}

#[must_use]
pub fn summary_lines_for_model(usage: TokenUsage, label: &str, model: Option<&str>) -> Vec<String> {
    let pricing = model.and_then(pricing_for_model);
    let cost = pricing.map_or_else(
        || estimate_cost_usd(usage),
        |pricing| estimate_cost_usd_with_pricing(usage, pricing),
    );
    let model_suffix = model.map_or_else(String::new, |model_name| format!(" model={model_name}"));
    let pricing_suffix = if pricing.is_some() {
        ""
    } else if model.is_some() {
        " pricing=estimated-default"
    } else {
        ""
    };
    vec![
        format!(
            "{label}: total_tokens={} input={} output={} cache_write={} cache_read={} estimated_cost={}{}{}",
            usage.total_tokens(),
            usage.input_tokens,
            usage.output_tokens,
            usage.cache_creation_input_tokens,
            usage.cache_read_input_tokens,
            format_usd(cost.total_cost_usd()),
            model_suffix,
            pricing_suffix,
        ),
        format!(
            "  cost breakdown: input={} output={} cache_write={} cache_read={}",
            format_usd(cost.input_cost_usd),
            format_usd(cost.output_cost_usd),
            format_usd(cost.cache_creation_cost_usd),
            format_usd(cost.cache_read_cost_usd),
        ),
    ]
}

fn cost_for_tokens(tokens: u32, usd_per_million_tokens: f64) -> f64 {
    f64::from(tokens) / 1_000_000.0 * usd_per_million_tokens
}

/// Per-child cost attribution for `/cost` breakdown.
#[derive(Debug, Clone)]
pub struct AgentCostReport {
    pub agent_id: String,
    pub direct_cost_usd: f64,
    pub turn_count: u32,
}

/// Build a flat per-child cost breakdown from a session's `child_sessions`.
///
/// Walks `child_sessions` (flat list) and computes cost via each child's
/// recorded usage. When a child session records its model, use model-specific
/// pricing; otherwise fall back to the default estimate.
#[must_use]
pub fn build_cost_breakdown(session: &crate::session::Session) -> Vec<AgentCostReport> {
    session
        .child_sessions
        .iter()
        .map(|child| {
            let mut tracker = UsageTracker::new();
            for message in &child.messages {
                if let Some(usage) = message.usage {
                    tracker.record(usage);
                }
            }
            let cost = child
                .model
                .as_deref()
                .and_then(pricing_for_model)
                .map_or_else(
                    || estimate_cost_usd(tracker.cumulative_usage()),
                    |pricing| estimate_cost_usd_with_pricing(tracker.cumulative_usage(), pricing),
                );
            AgentCostReport {
                agent_id: child.id.clone(),
                direct_cost_usd: cost.total_cost_usd(),
                turn_count: tracker.turns(),
            }
        })
        .collect()
}

#[must_use]
pub fn format_usd(amount: f64) -> String {
    format!("${amount:.4}")
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UsageTracker {
    latest_turn: TokenUsage,
    cumulative: TokenUsage,
    turns: u32,
}

impl UsageTracker {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn from_session(session: &Session) -> Self {
        let mut tracker = Self::new();
        for message in &session.messages {
            if let Some(usage) = message.usage {
                tracker.record(usage);
            }
        }
        tracker
    }

    pub fn record(&mut self, usage: TokenUsage) {
        self.latest_turn = usage;
        self.cumulative.input_tokens += usage.input_tokens;
        self.cumulative.output_tokens += usage.output_tokens;
        self.cumulative.cache_creation_input_tokens += usage.cache_creation_input_tokens;
        self.cumulative.cache_read_input_tokens += usage.cache_read_input_tokens;
        self.turns += 1;
    }

    #[must_use]
    pub fn current_turn_usage(&self) -> TokenUsage {
        self.latest_turn
    }

    #[must_use]
    pub fn cumulative_usage(&self) -> TokenUsage {
        self.cumulative
    }

    #[must_use]
    pub fn turns(&self) -> u32 {
        self.turns
    }
}

#[cfg(test)]
mod tests {
    use super::{
        estimate_cost_usd, estimate_cost_usd_with_pricing, format_usd, pricing_for_model,
        summary_lines_for_model, TokenUsage, UsageTracker,
    };
    use crate::session::{ContentBlock, ConversationMessage, MessageRole, Session};

    #[test]
    fn tracks_true_cumulative_usage() {
        let mut tracker = UsageTracker::new();
        tracker.record(TokenUsage {
            input_tokens: 10,
            output_tokens: 4,
            cache_creation_input_tokens: 2,
            cache_read_input_tokens: 1,
        });
        tracker.record(TokenUsage {
            input_tokens: 20,
            output_tokens: 6,
            cache_creation_input_tokens: 3,
            cache_read_input_tokens: 2,
        });

        assert_eq!(tracker.turns(), 2);
        assert_eq!(tracker.current_turn_usage().input_tokens, 20);
        assert_eq!(tracker.current_turn_usage().output_tokens, 6);
        assert_eq!(tracker.cumulative_usage().output_tokens, 10);
        assert_eq!(tracker.cumulative_usage().input_tokens, 30);
        assert_eq!(tracker.cumulative_usage().total_tokens(), 48);
    }

    #[test]
    fn computes_cost_summary_lines() {
        let usage = TokenUsage {
            input_tokens: 1_000_000,
            output_tokens: 500_000,
            cache_creation_input_tokens: 100_000,
            cache_read_input_tokens: 200_000,
        };

        let cost = estimate_cost_usd(usage);
        assert_eq!(format_usd(cost.input_cost_usd), "$15.0000");
        assert_eq!(format_usd(cost.output_cost_usd), "$37.5000");
        let lines = summary_lines_for_model(usage, "usage", Some("claude-sonnet-4-20250514"));
        assert!(lines[0].contains("estimated_cost=$54.6750"));
        assert!(lines[0].contains("model=claude-sonnet-4-20250514"));
        assert!(lines[1].contains("cache_read=$0.3000"));
    }

    #[test]
    fn supports_model_specific_pricing() {
        let usage = TokenUsage {
            input_tokens: 1_000_000,
            output_tokens: 500_000,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
        };

        let haiku = pricing_for_model("claude-haiku-4-5-20251001").expect("haiku pricing");
        let opus = pricing_for_model("claude-opus-4-6").expect("opus pricing");
        let haiku_cost = estimate_cost_usd_with_pricing(usage, haiku);
        let opus_cost = estimate_cost_usd_with_pricing(usage, opus);
        assert_eq!(format_usd(haiku_cost.total_cost_usd()), "$3.5000");
        assert_eq!(format_usd(opus_cost.total_cost_usd()), "$52.5000");
    }

    #[test]
    fn marks_unknown_model_pricing_as_fallback() {
        let usage = TokenUsage {
            input_tokens: 100,
            output_tokens: 100,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
        };
        let lines = summary_lines_for_model(usage, "usage", Some("custom-model"));
        assert!(lines[0].contains("pricing=estimated-default"));
    }

    #[test]
    fn reconstructs_usage_from_session_messages() {
        let session = Session {
            version: 1,
            model: None,
            title: None,
            messages: vec![ConversationMessage {
                role: MessageRole::Assistant,
                blocks: vec![ContentBlock::Text {
                    text: "done".to_string(),
                }],
                usage: Some(TokenUsage {
                    input_tokens: 5,
                    output_tokens: 2,
                    cache_creation_input_tokens: 1,
                    cache_read_input_tokens: 0,
                }),
            }],
            child_sessions: Vec::new(),
        };

        let tracker = UsageTracker::from_session(&session);
        assert_eq!(tracker.turns(), 1);
        assert_eq!(tracker.cumulative_usage().total_tokens(), 8);
    }

    #[test]
    fn build_cost_breakdown_computes_per_child_costs() {
        use super::build_cost_breakdown;
        use crate::session::ChildSession;

        let session = Session {
            version: 2,
            model: None,
            title: None,
            messages: Vec::new(),
            child_sessions: vec![
                ChildSession {
                    id: "child-a".to_string(),
                    model: Some("claude-haiku-4-5-20251001".to_string()),
                    goal: "scrape page A".to_string(),
                    messages: vec![
                        ConversationMessage {
                            role: MessageRole::Assistant,
                            blocks: vec![ContentBlock::Text {
                                text: "working".to_string(),
                            }],
                            usage: Some(TokenUsage {
                                input_tokens: 1_000_000,
                                output_tokens: 100_000,
                                cache_creation_input_tokens: 0,
                                cache_read_input_tokens: 0,
                            }),
                        },
                        ConversationMessage {
                            role: MessageRole::Assistant,
                            blocks: vec![ContentBlock::Text {
                                text: "done".to_string(),
                            }],
                            usage: Some(TokenUsage {
                                input_tokens: 500_000,
                                output_tokens: 50_000,
                                cache_creation_input_tokens: 0,
                                cache_read_input_tokens: 0,
                            }),
                        },
                    ],
                },
                ChildSession {
                    id: "child-b".to_string(),
                    model: None,
                    goal: "scrape page B".to_string(),
                    messages: vec![ConversationMessage {
                        role: MessageRole::User,
                        blocks: vec![ContentBlock::Text {
                            text: "go".to_string(),
                        }],
                        usage: None,
                    }],
                },
            ],
        };

        let breakdown = build_cost_breakdown(&session);
        assert_eq!(breakdown.len(), 2);

        // child-a: 2 assistant messages with usage
        assert_eq!(breakdown[0].agent_id, "child-a");
        assert_eq!(breakdown[0].turn_count, 2);
        assert_eq!(format_usd(breakdown[0].direct_cost_usd), "$2.2500");

        // child-b: 1 user message with no usage → 0 turns, 0 cost
        assert_eq!(breakdown[1].agent_id, "child-b");
        assert_eq!(breakdown[1].turn_count, 0);
        assert!(breakdown[1].direct_cost_usd.abs() < f64::EPSILON);
    }

    #[test]
    fn build_cost_breakdown_empty_when_no_children() {
        use super::build_cost_breakdown;

        let session = Session {
            version: 2,
            model: None,
            title: None,
            messages: vec![ConversationMessage {
                role: MessageRole::Assistant,
                blocks: vec![ContentBlock::Text {
                    text: "hello".to_string(),
                }],
                usage: Some(TokenUsage {
                    input_tokens: 100,
                    output_tokens: 50,
                    cache_creation_input_tokens: 0,
                    cache_read_input_tokens: 0,
                }),
            }],
            child_sessions: Vec::new(),
        };

        let breakdown = build_cost_breakdown(&session);
        assert!(breakdown.is_empty());
    }
}
