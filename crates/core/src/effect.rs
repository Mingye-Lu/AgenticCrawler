use serde_json::Value;

/// Control-flow instruction returned by a tool handler.
#[derive(Debug, Clone)]
pub enum ToolEffect {
    /// Tool produced a plain string reply (the most common case).
    Reply(String),
    /// Tool requests spawning a sub-agent with the given typed work packet.
    Spawn(CrawlTask),
    /// Tool requests waiting for sub-agents to finish.
    Wait(WaitSpec),
    /// Tool requests cancelling running sub-agents. Cancellation is abortive:
    /// the children are torn down immediately and their in-flight work is
    /// discarded.
    Cancel(CancelSpec),
    /// Tool requests a read-only snapshot of running sub-agents. Never joins
    /// or cancels — safe to call between steps.
    Status(StatusSpec),

}

impl ToolEffect {
    #[must_use]
    pub fn reply_json(value: &Value) -> Self {
        Self::Reply(value.to_string())
    }
}

/// A typed, validated work packet for a forked sub-agent. The `scope`
/// declares which URLs the child is allowed to claim; siblings cannot fork
/// onto overlapping scope. This is the atomic dispatch primitive that
/// replaces the free-form `{ sub_goal: String }` of older versions.
#[derive(Debug, Clone)]
pub struct CrawlTask {
    pub objective: String,
    pub scope: CrawlScope,
    pub max_steps: Option<usize>,
    /// Set by the fork supervisor once a page is allocated; never provided
    /// by the LLM.
    pub page_index: Option<usize>,
}

/// Declared work boundary for a sub-agent. The fork supervisor refuses to
/// dispatch overlapping scopes (same URL, overlapping pattern) so siblings
/// don't redo each other's work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CrawlScope {
    /// Crawl one specific URL.
    SinglePage { url: String },
    /// Crawl an explicit list of URLs (≥ 1).
    UrlList { urls: Vec<String> },
    /// Crawl any URL matching this regex. The pattern source string is used
    /// as the conflict key — siblings cannot request the same pattern, and
    /// the pattern cannot overlap with already-claimed exact URLs.
    UrlPattern { regex: String },
}

/// Parameters for waiting on sub-agents.
#[derive(Debug, Clone)]
pub struct WaitSpec {
    pub child_ids: Option<Vec<String>>,
}

/// Parameters for explicitly cancelling sub-agents.
#[derive(Debug, Clone)]
pub struct CancelSpec {
    pub child_ids: Vec<String>,
    pub reason: Option<String>,
}

/// Parameters for polling sub-agent status without joining them.
#[derive(Debug, Clone)]
pub struct StatusSpec {
    pub child_ids: Option<Vec<String>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reply_json_serializes_complex_value() {
        let value = serde_json::json!({"items": [1, 2, 3], "nested": {"key": "val"}});
        let effect = ToolEffect::reply_json(&value);
        match effect {
            ToolEffect::Reply(s) => {
                let parsed: serde_json::Value =
                    serde_json::from_str(&s).expect("should be valid JSON");
                assert_eq!(parsed["items"][0], 1);
                assert_eq!(parsed["nested"]["key"], "val");
            }
            _ => panic!("expected Reply variant"),
        }
    }

    #[test]
    fn reply_json_roundtrips_null() {
        let effect = ToolEffect::reply_json(&serde_json::json!(null));
        match effect {
            ToolEffect::Reply(s) => assert_eq!(s, "null"),
            _ => panic!("expected Reply variant"),
        }
    }


}
