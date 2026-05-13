use ratatui::widgets::ListState;

use crate::markdown::PredictiveMarkdownBuffer;
use super::repl_app::{ToolCallStatus, TranscriptEntry};
use super::repl_render::ansi_to_lines;

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
    pub(super) sub_goal: String,
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
    pub(super) md_buffer: PredictiveMarkdownBuffer,
    pub(super) live_ansi: String,
    pub(super) scroll_offset: usize,
    pub(super) scrollback: Vec<String>,
}

impl ChildTabState {
    pub fn new(child_id: String, sub_goal: String) -> Self {
        Self {
            child_id,
            sub_goal,
            status: ChildTabStatus::Running,
            step: 0,
            max_steps: 0,
            tool_in_progress: None,
            items_extracted: 0,
            follow_bottom: true,
            entries: Vec::new(),
            list_state: ListState::default(),
            last_wrapped_len: 0,
            last_view_height: 0,
            md_buffer: PredictiveMarkdownBuffer::new(),
            live_ansi: String::new(),
            scroll_offset: 0,
            scrollback: Vec::new(),
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
    pub fn apply_event(&mut self, child_id: &str, sub_goal: &str, event: &crawler::ChildEventKind) {
        let idx = self.get_or_create_tab(child_id, sub_goal);

        match event {
            crawler::ChildEventKind::TextDelta(text) => {
                let tab = &mut self.tabs[idx];
                for c in text.chars() {
                    tab.md_buffer.feed_char(c, &mut tab.live_ansi);
                    if c == '\n' {
                        let ansi = std::mem::take(&mut tab.live_ansi);
                        for styled_line in ansi_to_lines(&ansi) {
                            tab.entries.push(TranscriptEntry::Stream(styled_line));
                        }
                    }
                }
            }
            crawler::ChildEventKind::ToolCallStart {
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
            crawler::ChildEventKind::ToolCallComplete {
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
            crawler::ChildEventKind::StepStarted { step, max_steps } => {
                let tab = &mut self.tabs[idx];
                tab.step = *step;
                tab.max_steps = *max_steps;
                tab.entries
                    .push(TranscriptEntry::Status(format!("Step {step}/{max_steps}")));
            }
            crawler::ChildEventKind::PauseRequested { reason } => {
                self.tabs[idx].status = ChildTabStatus::Paused {
                    reason: reason.clone(),
                };
                self.tabs[idx]
                    .entries
                    .push(TranscriptEntry::System(format!("⏸ Paused: {reason}")));
                self.active_tab = idx;
            }
            crawler::ChildEventKind::Resumed => {
                let tab = &mut self.tabs[idx];
                tab.status = ChildTabStatus::Running;
                tab.entries
                    .push(TranscriptEntry::System("▶ Resumed".to_string()));
            }
            crawler::ChildEventKind::Finished {
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
                        error
                            .clone()
                            .unwrap_or_else(|| "unknown error".to_string()),
                    )
                };
                tab.tool_in_progress = None;

                tab.md_buffer.flush(&mut tab.live_ansi);
                if !tab.live_ansi.is_empty() {
                    let ansi = std::mem::take(&mut tab.live_ansi);
                    for styled_line in ansi_to_lines(&ansi) {
                        tab.entries.push(TranscriptEntry::Stream(styled_line));
                    }
                }

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
                    format!("✓ Done — {items_extracted} items extracted")
                } else {
                    format!("✗ Error: {}", error.as_deref().unwrap_or("unknown error"))
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
            &crawler::ChildEventKind::StepStarted {
                step: 1,
                max_steps: 5,
            },
        );
        assert_eq!(panel.tabs[0].status, ChildTabStatus::Running);

        // Pause it
        panel.apply_event(
            "agent-a",
            "fetch data",
            &crawler::ChildEventKind::PauseRequested {
                reason: "rate limit".into(),
            },
        );
        assert!(matches!(
            panel.tabs[0].status,
            ChildTabStatus::Paused { .. }
        ));
        assert!(panel.active_tab_is_paused());

        // Resume it
        panel.apply_event("agent-a", "fetch data", &crawler::ChildEventKind::Resumed);
        assert_eq!(panel.tabs[0].status, ChildTabStatus::Running);
        assert!(!panel.active_tab_is_paused());

        // Finish it
        panel.apply_event(
            "agent-a",
            "fetch data",
            &crawler::ChildEventKind::Finished {
                success: true,
                items_extracted: 10,
                error: None,
            },
        );
        assert_eq!(panel.tabs[0].status, ChildTabStatus::Done);
        assert_eq!(panel.tabs[0].items_extracted, 10);
        assert!(matches!(panel.tabs[0].entries.last(), Some(TranscriptEntry::System(message)) if message.contains("Done") || message.contains('✓')));
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
            &crawler::ChildEventKind::PauseRequested {
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
            &crawler::ChildEventKind::PauseRequested {
                reason: "r1".into(),
            },
        );
        panel.apply_event(
            "c2",
            "g2",
            &crawler::ChildEventKind::PauseRequested {
                reason: "r2".into(),
            },
        );
        panel.apply_event("c1", "g1", &crawler::ChildEventKind::Resumed);
        assert_eq!(panel.tabs[0].status, ChildTabStatus::Running);
        assert!(matches!(
            panel.tabs[1].status,
            ChildTabStatus::Paused { .. }
        ));
    }
}
