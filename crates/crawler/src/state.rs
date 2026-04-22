use serde_json::Value;

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
    pub done: bool,
    pub done_reason: String,
}

impl CrawlState {
    /// Create a child state for a sub-goal with deep-copied history and reset transient fields.
    ///
    /// # Arguments
    /// * `sub_goal` - The goal for the child state
    /// * `url` - Optional URL for the child state; inherits parent's `current_url` if not provided
    /// * `child_max_steps` - Maximum steps for the child state
    ///
    /// # Returns
    /// A new `CrawlState` with copied history and reset transient fields
    #[must_use]
    pub fn fork(&self, _sub_goal: &str, url: Option<&str>, child_max_steps: usize) -> CrawlState {
        CrawlState {
            current_url: url.map(str::to_string).or_else(|| self.current_url.clone()),
            action_history: self.action_history.clone(),
            extracted_data: Vec::new(),
            step_count: 0,
            child_blocks: Vec::new(),
            max_steps: child_max_steps,
            done: false,
            done_reason: String::new(),
        }
    }

    /// Return all extracted data (own + children) as a flat list.
    #[must_use]
    pub fn all_data(&self) -> Vec<Value> {
        let mut result = self.extracted_data.clone();
        for block in &self.child_blocks {
            result.extend(block.items.clone());
        }
        result
    }
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

        // Verify history is copied
        assert_eq!(child.action_history, parent.action_history);

        // Verify it's a deep copy by mutating parent and checking child is unchanged
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
            done: true,
            done_reason: "parent done".to_string(),
            child_blocks: vec![ChildBlock {
                child_id: "child1".to_string(),
                sub_goal: "goal1".to_string(),
                items: vec![],
            }],
            ..CrawlState::default()
        };

        let child = parent.fork("child_goal", None, 10);

        // Verify transient fields are reset
        assert_eq!(child.extracted_data.len(), 0);
        assert_eq!(child.step_count, 0);
        assert!(!child.done);
        assert_eq!(child.done_reason, "");
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
            extracted_data: vec![
                serde_json::json!({"id": 1}),
                serde_json::json!({"id": 2}),
            ],
            child_blocks: vec![
                ChildBlock {
                    child_id: "child1".to_string(),
                    sub_goal: "goal1".to_string(),
                    items: vec![serde_json::json!({"id": 3})],
                },
                ChildBlock {
                    child_id: "child2".to_string(),
                    sub_goal: "goal2".to_string(),
                    items: vec![
                        serde_json::json!({"id": 4}),
                        serde_json::json!({"id": 5}),
                    ],
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

        // Mutate child's history
        child.action_history.push("child_action".to_string());

        // Verify parent is unchanged
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
}
