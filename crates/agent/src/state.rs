use std::collections::HashMap;
use std::sync::Arc;

use browser::ConsoleMessageEvent;
use browser::NetworkRequestEvent;
use browser::ObservationEvent;
use browser::WebSocketFrameEvent;
use runtime::ChildSession;
use serde_json::Value;

use crate::action_cache::ActionCache;
use crate::aria::AriaNode;
use crate::loop_detector::LoopDetector;
use crate::page_fingerprint::PageFingerprint;
use crate::tools::html_diff::HtmlDiffTracker;
use crate::tools::websocket_activity::WebSocketConnectionRef;

#[derive(Debug, Clone)]
pub struct ChildBlock {
    pub child_id: String,
    pub sub_goal: String,
    pub items: Vec<Value>,
}

#[derive(Debug, Clone, Default)]
pub struct CrawlState {
    pub current_url: Option<String>,
    pub action_history: Vec<String>,
    pub extracted_data: Vec<Value>,
    pub step_count: usize,
    pub child_blocks: Vec<ChildBlock>,
    pub max_steps: usize,
    pub captured_child_sessions: Vec<ChildSession>,
    pub page_fingerprints: Vec<PageFingerprint>,
    /// Most recent ARIA tree emitted by `page_map`/`navigate`. The next snapshot
    /// reconciles against it so unchanged nodes keep their `@eN` refs, and it
    /// feeds `PageFingerprint::compute`.
    pub last_aria_tree: Option<AriaNode>,
    pub action_cache: Option<ActionCache>,
    pub html_diff_tracker: Option<HtmlDiffTracker>,
    pub loop_detector: Option<LoopDetector>,
    pub page_log_events: Vec<ConsoleMessageEvent>,
    pub page_log_groups: HashMap<String, Vec<ConsoleMessageEvent>>,
    pub last_page_log_seq: Option<u64>,
    pub network_request_events: Vec<NetworkRequestEvent>,
    pub network_request_refs: HashMap<String, NetworkRequestEvent>,
    pub websocket_frame_events: Vec<WebSocketFrameEvent>,
    pub websocket_connection_refs: HashMap<String, WebSocketConnectionRef>,
    /// Current device emulation mode. None = desktop default.
    pub current_device: Option<String>,
    /// Whether any sub-agents are currently running. Updated before each tool call.
    pub has_active_subagents: bool,
    /// Global monotonic sequence counter shared across forked agents.
    /// Used by action tools to tag responses for temporal observation filtering.
    pub seq_counter: Arc<browser::SeqCounter>,
    /// Active network interception rules: (`rule_id`, pattern, `action_name`)
    pub intercept_rules: Vec<(String, String, String)>,
}

impl CrawlState {
    #[must_use]
    pub fn fork(&self, _sub_goal: &str, url: Option<&str>, child_max_steps: usize) -> CrawlState {
        CrawlState {
            current_url: url.map(str::to_string).or_else(|| self.current_url.clone()),
            action_history: self.action_history.clone(),
            extracted_data: Vec::new(),
            step_count: 0,
            child_blocks: Vec::new(),
            max_steps: child_max_steps,
            captured_child_sessions: Vec::new(),
            page_fingerprints: Vec::new(),
            last_aria_tree: None,
            action_cache: None,
            html_diff_tracker: None,
            loop_detector: None,
            page_log_events: Vec::new(),
            page_log_groups: HashMap::new(),
            last_page_log_seq: None,
            network_request_events: Vec::new(),
            network_request_refs: HashMap::new(),
            websocket_frame_events: Vec::new(),
            websocket_connection_refs: HashMap::new(),
            current_device: None,
            has_active_subagents: false,
            seq_counter: Arc::clone(&self.seq_counter),
            intercept_rules: Vec::new(),
        }
    }

    #[must_use]
    pub fn all_data(&self) -> Vec<Value> {
        let mut result = self.extracted_data.clone();
        for block in &self.child_blocks {
            result.extend(block.items.clone());
        }
        result
    }

    /// Route a freshly polled observation batch into the per-type stores.
    ///
    /// `poll_observations` drains and clears the bridge's single shared buffer,
    /// so whichever observation tool polls first receives every event type.
    /// Routing all types here (instead of each tool keeping only its own and
    /// discarding the rest) stops one tool's poll from silently dropping another
    /// type's events before the matching tool reads them.
    pub fn ingest_observations(&mut self, polled: Vec<ObservationEvent>) {
        for event in polled {
            match event {
                ObservationEvent::NetworkRequest(network) => {
                    if !is_internal_observation_url(&network.url) {
                        self.network_request_events.push(*network);
                    }
                }
                ObservationEvent::ConsoleMessage(console) => {
                    self.page_log_events.push(console);
                }
                ObservationEvent::WebSocketFrame(frame) => {
                    self.websocket_frame_events.push(frame);
                }
            }
        }
    }
}

fn is_internal_observation_url(url: &str) -> bool {
    url.contains("__acrawl_poll") || url.contains("poll_observations")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_crawl_state_fork_deep_copies_history() {
        let parent = CrawlState {
            action_history: vec!["action1".to_string(), "action2".to_string()],
            ..CrawlState::default()
        };

        let child = parent.fork("child_goal", None, 10);

        assert_eq!(child.action_history, parent.action_history);

        let mut parent_mut = parent.clone();
        parent_mut.action_history.push("action3".to_string());
        assert_eq!(child.action_history.len(), 2);
        assert_eq!(parent_mut.action_history.len(), 3);
    }

    #[test]
    fn test_crawl_state_fork_resets_transient_fields() {
        let parent = CrawlState {
            extracted_data: vec![serde_json::json!({"key": "value"})],
            step_count: 5,
            child_blocks: vec![ChildBlock {
                child_id: "child1".to_string(),
                sub_goal: "goal1".to_string(),
                items: vec![],
            }],
            ..CrawlState::default()
        };

        let child = parent.fork("child_goal", None, 10);

        assert_eq!(child.extracted_data.len(), 0);
        assert_eq!(child.step_count, 0);
        assert_eq!(child.child_blocks.len(), 0);
    }

    #[test]
    fn test_crawl_state_fork_inherits_url() {
        let parent = CrawlState {
            current_url: Some("https://example.com".to_string()),
            ..CrawlState::default()
        };

        let child = parent.fork("child_goal", None, 10);

        assert_eq!(child.current_url, Some("https://example.com".to_string()));
    }

    #[test]
    fn test_crawl_state_fork_uses_provided_url() {
        let parent = CrawlState {
            current_url: Some("https://example.com".to_string()),
            ..CrawlState::default()
        };

        let child = parent.fork("child_goal", Some("https://other.com"), 10);

        assert_eq!(child.current_url, Some("https://other.com".to_string()));
    }

    #[test]
    fn test_crawl_state_all_data_includes_children() {
        let parent = CrawlState {
            extracted_data: vec![serde_json::json!({"id": 1}), serde_json::json!({"id": 2})],
            child_blocks: vec![
                ChildBlock {
                    child_id: "child1".to_string(),
                    sub_goal: "goal1".to_string(),
                    items: vec![serde_json::json!({"id": 3})],
                },
                ChildBlock {
                    child_id: "child2".to_string(),
                    sub_goal: "goal2".to_string(),
                    items: vec![serde_json::json!({"id": 4}), serde_json::json!({"id": 5})],
                },
            ],
            ..CrawlState::default()
        };

        let all_data = parent.all_data();

        assert_eq!(all_data.len(), 5);
        assert_eq!(all_data[0], serde_json::json!({"id": 1}));
        assert_eq!(all_data[1], serde_json::json!({"id": 2}));
        assert_eq!(all_data[2], serde_json::json!({"id": 3}));
        assert_eq!(all_data[3], serde_json::json!({"id": 4}));
        assert_eq!(all_data[4], serde_json::json!({"id": 5}));
    }

    #[test]
    fn test_crawl_state_fork_isolation() {
        let parent = CrawlState {
            action_history: vec!["parent_action".to_string()],
            ..CrawlState::default()
        };

        let mut child = parent.fork("child_goal", None, 10);

        child.action_history.push("child_action".to_string());

        assert_eq!(parent.action_history.len(), 1);
        assert_eq!(child.action_history.len(), 2);
    }

    #[test]
    fn test_crawl_state_fork_sets_max_steps() {
        let parent = CrawlState::default();
        let child = parent.fork("child_goal", None, 25);

        assert_eq!(child.max_steps, 25);
    }

    #[test]
    fn test_crawl_state_all_data_empty() {
        let state = CrawlState::default();
        let all_data = state.all_data();

        assert_eq!(all_data.len(), 0);
    }

    #[test]
    fn ingest_observations_routes_each_type_and_drops_internal_requests() {
        use browser::{
            ConsoleMessageEvent, ConsoleMessageType, NetworkRequestEvent, ObservationEvent,
            RequestState, WebSocketFrameEvent,
        };

        let net = |url: &str| {
            ObservationEvent::NetworkRequest(Box::new(NetworkRequestEvent {
                timestamp_ms: 1,
                tab_index: 0,
                seq_at_initiation: 1,
                request_id: "r".to_string(),
                url: url.to_string(),
                method: "GET".to_string(),
                status: Some(200),
                state: RequestState::Completed,
                size_bytes: None,
                duration_ms: None,
                request_type: "fetch".to_string(),
                from_service_worker: false,
                initiator_type: None,
                reason: None,
                timing: None,
                request_headers: None,
                response_headers: None,
                request_body: None,
                response_body: None,
            }))
        };
        let console = ObservationEvent::ConsoleMessage(ConsoleMessageEvent {
            timestamp_ms: 2,
            tab_index: 0,
            seq_at_initiation: 1,
            level: "error".to_string(),
            message_type: ConsoleMessageType::Exception,
            text: "boom".to_string(),
            source_url: None,
            source_line: None,
            source_column: None,
            stack: None,
        });
        let ws = ObservationEvent::WebSocketFrame(WebSocketFrameEvent {
            timestamp_ms: 3,
            tab_index: 0,
            seq_at_initiation: 1,
            connection_id: "c1".to_string(),
            url: "wss://ex.com".to_string(),
            direction: "received".to_string(),
            data: "hi".to_string(),
            size_bytes: 2,
            connection_status: "open".to_string(),
        });

        let mut state = CrawlState::default();
        state.ingest_observations(vec![
            net("https://example.com/api"),
            net("https://example.com/__acrawl_poll"),
            console,
            ws,
        ]);

        assert_eq!(state.network_request_events.len(), 1);
        assert_eq!(
            state.network_request_events[0].url,
            "https://example.com/api"
        );
        assert_eq!(state.page_log_events.len(), 1);
        assert_eq!(state.page_log_events[0].text, "boom");
        assert_eq!(state.websocket_frame_events.len(), 1);
        assert_eq!(state.websocket_frame_events[0].data, "hi");
    }
}
