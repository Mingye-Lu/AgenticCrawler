use agent::ChildEventKind;
use ratatui::widgets::ListState;

use super::repl_app::{ToolCallStatus, TranscriptEntry};
use crate::markdown::{drain_safe_boundary, render_lines};

const MAX_ENTRIES: usize = 1000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChildTabStatus {
    Running,
    Paused { reason: String },
    Done,
    Error(String),
}

#[derive(Clone)]
pub struct ChildTabState {
    pub(super) child_id: String,
    pub(super) status: ChildTabStatus,
    pub(super) step: usize,
    pub(super) max_steps: usize,
    pub(super) tool_in_progress: Option<String>,
    pub(super) items_extracted: usize,
    pub(super) follow_bottom: bool,
    pub(super) entries: Vec<TranscriptEntry>,
    pub(super) list_state: ListState,
    pub(super) last_wrapped_len: usize,
    pub(super) last_view_height: usize,
    pub(super) live: String,
}

impl ChildTabState {
    pub fn new(child_id: String, sub_goal: String) -> Self {
        let entries = vec![TranscriptEntry::Parent(sub_goal)];
        Self {
            child_id,
            status: ChildTabStatus::Running,
            step: 0,
            max_steps: 0,
            tool_in_progress: None,
            items_extracted: 0,
            follow_bottom: true,
            entries,
            list_state: ListState::default(),
            last_wrapped_len: 0,
            last_view_height: 0,
            live: String::new(),
        }
    }

    fn drain_completed_lines(&mut self) {
        while let Some(styled_lines) = drain_safe_boundary(&mut self.live) {
            for line in styled_lines {
                self.entries.push(TranscriptEntry::Stream(line));
            }
        }
    }

    fn flush_remainder(&mut self) {
        if self.live.is_empty() {
            return;
        }
        let pending = std::mem::take(&mut self.live);
        for styled_line in render_lines(&pending) {
            self.entries.push(TranscriptEntry::Stream(styled_line));
        }
    }
}

#[derive(Default)]
pub struct ChildTabPanel {
    pub tabs: Vec<ChildTabState>,
    pub active_tab: usize,
}

impl ChildTabPanel {
    pub fn get_or_create_tab(&mut self, child_id: &str, sub_goal: &str) -> usize {
        if let Some(idx) = self.tabs.iter().position(|t| t.child_id == child_id) {
            return idx;
        }
        self.tabs.push(ChildTabState::new(
            child_id.to_string(),
            sub_goal.to_string(),
        ));
        self.active_tab = self.tabs.len() - 1;
        self.tabs.len() - 1
    }

    pub fn next_tab(&mut self) {
        if !self.tabs.is_empty() {
            self.active_tab = (self.active_tab + 1) % self.tabs.len();
        }
    }

    pub fn prev_tab(&mut self) {
        if !self.tabs.is_empty() {
            self.active_tab = if self.active_tab == 0 {
                self.tabs.len() - 1
            } else {
                self.active_tab - 1
            };
        }
    }

    pub fn active_tab_state_mut(&mut self) -> Option<&mut ChildTabState> {
        self.tabs.get_mut(self.active_tab)
    }

    pub fn find_tab_mut(&mut self, child_id: &str) -> Option<&mut ChildTabState> {
        self.tabs.iter_mut().find(|t| t.child_id == child_id)
    }

    #[allow(clippy::too_many_lines)]
    pub fn apply_event(&mut self, child_id: &str, sub_goal: &str, event: &ChildEventKind) {
        let idx = self.get_or_create_tab(child_id, sub_goal);

        match event {
            ChildEventKind::TextDelta(text) => {
                let tab = &mut self.tabs[idx];
                tab.live.push_str(text);
                tab.drain_completed_lines();
            }
            ChildEventKind::ToolCallStart {
                name,
                input_summary,
            } => {
                let tab = &mut self.tabs[idx];
                tab.tool_in_progress = Some(name.clone());
                tab.entries.push(TranscriptEntry::ToolCall {
                    name: name.clone(),
                    input_summary: input_summary.clone(),
                    status: ToolCallStatus::Running,
                });
            }
            ChildEventKind::ToolCallComplete {
                name,
                output_summary,
                is_error,
            } => {
                let tab = &mut self.tabs[idx];
                tab.tool_in_progress = None;
                let completed_status = if *is_error {
                    ToolCallStatus::Error(output_summary.clone())
                } else {
                    ToolCallStatus::Success {
                        output: output_summary.clone(),
                    }
                };

                let updated = tab.entries.iter_mut().rev().find_map(|entry| match entry {
                    TranscriptEntry::ToolCall {
                        name: entry_name,
                        status: status @ ToolCallStatus::Running,
                        ..
                    } if entry_name == name => Some(status),
                    _ => None,
                });

                if let Some(status) = updated {
                    *status = completed_status;
                } else {
                    tab.entries.push(TranscriptEntry::ToolCall {
                        name: name.clone(),
                        input_summary: String::new(),
                        status: completed_status,
                    });
                }
            }
            ChildEventKind::StepStarted { step, max_steps } => {
                let tab = &mut self.tabs[idx];
                tab.step = *step;
                tab.max_steps = *max_steps;
            }
            ChildEventKind::PauseRequested { reason } => {
                self.tabs[idx].status = ChildTabStatus::Paused {
                    reason: reason.clone(),
                };
                self.tabs[idx]
                    .entries
                    .push(TranscriptEntry::System(format!("PAUSED: {reason}")));
                self.active_tab = idx;
            }
            ChildEventKind::Resumed => {
                let tab = &mut self.tabs[idx];
                tab.status = ChildTabStatus::Running;
                tab.entries
                    .push(TranscriptEntry::System("Resumed".to_string()));
            }
            ChildEventKind::Finished {
                success,
                items_extracted,
                error,
            } => {
                let tab = &mut self.tabs[idx];
                tab.items_extracted = *items_extracted;
                tab.status = if *success {
                    ChildTabStatus::Done
                } else {
                    ChildTabStatus::Error(
                        error.clone().unwrap_or_else(|| "unknown error".to_string()),
                    )
                };
                tab.tool_in_progress = None;
                tab.drain_completed_lines();
                tab.flush_remainder();

                for entry in &mut tab.entries {
                    if let TranscriptEntry::ToolCall {
                        status: status @ ToolCallStatus::Running,
                        ..
                    } = entry
                    {
                        *status = ToolCallStatus::Error("interrupted".to_string());
                    }
                }

                let message = if *success {
                    format!("鉁?Done -- {items_extracted} items extracted")
                } else {
                    format!("鉁?Error: {}", error.as_deref().unwrap_or("unknown error"))
                };
                tab.entries.push(TranscriptEntry::System(message));
            }
        }

        if self.tabs[idx].entries.len() > MAX_ENTRIES {
            let excess = self.tabs[idx].entries.len() - MAX_ENTRIES;
            self.tabs[idx].entries.drain(0..excess);
        }
    }

    pub fn active_tab_is_paused(&self) -> bool {
        self.tabs
            .get(self.active_tab)
            .is_some_and(|t| matches!(t.status, ChildTabStatus::Paused { .. }))
    }

    pub fn active_child_id(&self) -> Option<&str> {
        self.tabs.get(self.active_tab).map(|t| t.child_id.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn child_tab_panel_event_state_transitions() {
        let mut panel = ChildTabPanel::default();
        panel.get_or_create_tab("agent-a", "fetch data");

        // Start running
        panel.apply_event(
            "agent-a",
            "fetch data",
            &ChildEventKind::StepStarted {
                step: 1,
                max_steps: 5,
            },
        );
        assert_eq!(panel.tabs[0].status, ChildTabStatus::Running);

        // Pause it
        panel.apply_event(
            "agent-a",
            "fetch data",
            &ChildEventKind::PauseRequested {
                reason: "rate limit".into(),
            },
        );
        assert!(matches!(
            panel.tabs[0].status,
            ChildTabStatus::Paused { .. }
        ));
        assert!(panel.active_tab_is_paused());

        // Resume it
        panel.apply_event("agent-a", "fetch data", &ChildEventKind::Resumed);
        assert_eq!(panel.tabs[0].status, ChildTabStatus::Running);
        assert!(!panel.active_tab_is_paused());

        // Finish it
        panel.apply_event(
            "agent-a",
            "fetch data",
            &ChildEventKind::Finished {
                success: true,
                items_extracted: 10,
                error: None,
            },
        );
        assert_eq!(panel.tabs[0].status, ChildTabStatus::Done);
        assert_eq!(panel.tabs[0].items_extracted, 10);
        assert!(
            matches!(panel.tabs[0].entries.last(), Some(TranscriptEntry::System(message)) if message.contains("Done"))
        );
    }

    #[test]
    fn get_or_create_tab_no_duplicates() {
        let mut panel = ChildTabPanel::default();
        let i1 = panel.get_or_create_tab("c1", "g1");
        let i2 = panel.get_or_create_tab("c1", "g1");
        assert_eq!(i1, i2);
        assert_eq!(panel.tabs.len(), 1);
    }

    #[test]
    fn pause_switches_active_tab() {
        let mut panel = ChildTabPanel::default();
        panel.get_or_create_tab("c1", "g1");
        panel.get_or_create_tab("c2", "g2");
        panel.active_tab = 0;
        panel.apply_event(
            "c2",
            "g2",
            &ChildEventKind::PauseRequested {
                reason: "captcha".into(),
            },
        );
        assert_eq!(panel.active_tab, 1);
        assert!(panel.active_tab_is_paused());
    }

    #[test]
    fn two_children_pause_independently() {
        let mut panel = ChildTabPanel::default();
        panel.get_or_create_tab("c1", "g1");
        panel.get_or_create_tab("c2", "g2");
        panel.apply_event(
            "c1",
            "g1",
            &ChildEventKind::PauseRequested {
                reason: "r1".into(),
            },
        );
        panel.apply_event(
            "c2",
            "g2",
            &ChildEventKind::PauseRequested {
                reason: "r2".into(),
            },
        );
        panel.apply_event("c1", "g1", &ChildEventKind::Resumed);
        assert_eq!(panel.tabs[0].status, ChildTabStatus::Running);
        assert!(matches!(
            panel.tabs[1].status,
            ChildTabStatus::Paused { .. }
        ));
    }

    #[test]
    fn test_tool_call_start_produces_running_entry() {
        let mut panel = ChildTabPanel::default();
        panel.apply_event(
            "child-1",
            "goal",
            &ChildEventKind::ToolCallStart {
                name: "navigate".to_string(),
                input_summary: "https://example.com".to_string(),
            },
        );
        let tab = &panel.tabs[0];
        // entries[0] is the initial Parent entry; entries[1] is the ToolCall
        assert_eq!(tab.entries.len(), 2);
        match &tab.entries[1] {
            TranscriptEntry::ToolCall { name, status, .. } => {
                assert_eq!(name, "navigate");
                assert!(matches!(status, ToolCallStatus::Running));
            }
            _ => panic!("Expected ToolCall(Running) entry"),
        }
    }

    #[test]
    fn test_tool_call_complete_updates_to_success() {
        let mut panel = ChildTabPanel::default();
        panel.apply_event(
            "child-1",
            "goal",
            &ChildEventKind::ToolCallStart {
                name: "navigate".to_string(),
                input_summary: "url".to_string(),
            },
        );
        panel.apply_event(
            "child-1",
            "goal",
            &ChildEventKind::ToolCallComplete {
                name: "navigate".to_string(),
                output_summary: "loaded page".to_string(),
                is_error: false,
            },
        );
        let tab = &panel.tabs[0];
        // entries[0] is the initial Parent entry; entries[1] is the ToolCall
        match &tab.entries[1] {
            TranscriptEntry::ToolCall { status, .. } => {
                assert!(matches!(status, ToolCallStatus::Success { .. }));
            }
            _ => panic!("Expected ToolCall(Success) entry"),
        }
    }

    #[test]
    fn test_tool_call_complete_updates_to_error() {
        let mut panel = ChildTabPanel::default();
        panel.apply_event(
            "child-1",
            "goal",
            &ChildEventKind::ToolCallStart {
                name: "click".to_string(),
                input_summary: "#btn".to_string(),
            },
        );
        panel.apply_event(
            "child-1",
            "goal",
            &ChildEventKind::ToolCallComplete {
                name: "click".to_string(),
                output_summary: "element not found".to_string(),
                is_error: true,
            },
        );
        let tab = &panel.tabs[0];
        // entries[0] is the initial Parent entry; entries[1] is the ToolCall
        match &tab.entries[1] {
            TranscriptEntry::ToolCall { status, .. } => {
                assert!(matches!(status, ToolCallStatus::Error(_)));
            }
            _ => panic!("Expected ToolCall(Error) entry"),
        }
    }

    #[test]
    fn test_text_delta_splits_on_newline() {
        let mut panel = ChildTabPanel::default();
        panel.apply_event(
            "child-1",
            "goal",
            &ChildEventKind::TextDelta("hello\nworld".to_string()),
        );
        let tab = &panel.tabs[0];
        let stream_count = tab
            .entries
            .iter()
            .filter(|e| matches!(e, TranscriptEntry::Stream(_)))
            .count();
        assert!(
            stream_count >= 1,
            "Expected at least 1 Stream entry from 'hello\\n' split, got {stream_count}"
        );
    }

    #[test]
    fn test_finished_converts_running_to_interrupted() {
        let mut panel = ChildTabPanel::default();
        panel.apply_event(
            "child-1",
            "goal",
            &ChildEventKind::ToolCallStart {
                name: "navigate".to_string(),
                input_summary: "url".to_string(),
            },
        );
        // Don't complete the tool 鈥?let Finished interrupt it
        panel.apply_event(
            "child-1",
            "goal",
            &ChildEventKind::Finished {
                success: false,
                items_extracted: 0,
                error: Some("interrupted".to_string()),
            },
        );
        let tab = &panel.tabs[0];
        let interrupted = tab.entries.iter().any(|e| {
            matches!(e, TranscriptEntry::ToolCall {
                status: ToolCallStatus::Error(msg),
                ..
            } if msg == "interrupted")
        });
        assert!(
            interrupted,
            "Expected Running tool to become Error('interrupted') on Finished"
        );
    }

    #[test]
    fn test_bounded_storage_cap() {
        let mut panel = ChildTabPanel::default();
        // Push more than 1000 entries via TextDelta events (one line each)
        for i in 0..1100u32 {
            panel.apply_event(
                "child-1",
                "goal",
                &ChildEventKind::TextDelta(format!("line {i}\n")),
            );
        }
        let tab = &panel.tabs[0];
        assert!(
            tab.entries.len() <= 1000,
            "Expected entries to be capped at 1000, got {}",
            tab.entries.len()
        );
    }
}
