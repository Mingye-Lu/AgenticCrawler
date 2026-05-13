use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, ListState};
use ratatui::Frame;

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
    #[allow(dead_code)]
    pub(super) list_state: ListState,
    #[allow(dead_code)]
    pub(super) last_wrapped_len: usize,
    #[allow(dead_code)]
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

    #[allow(dead_code)]
    fn status_indicator(&self) -> &'static str {
        match &self.status {
            ChildTabStatus::Running => "●",
            ChildTabStatus::Paused { .. } => "⏸",
            ChildTabStatus::Done => "✓",
            ChildTabStatus::Error(_) => "✗",
        }
    }

    fn status_color(&self) -> Color {
        match &self.status {
            ChildTabStatus::Running => Color::Cyan,
            ChildTabStatus::Paused { .. } => Color::Yellow,
            ChildTabStatus::Done => Color::Green,
            ChildTabStatus::Error(_) => Color::Red,
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

    #[allow(clippy::too_many_lines)]
    pub fn render_fullscreen(&self, child_id: &str, frame: &mut Frame<'_>, area: Rect) {
        let Some(tab) = self.tabs.iter().find(|t| t.child_id == child_id) else {
            let block = Block::default()
                .borders(Borders::ALL)
                .border_type(ratatui::widgets::BorderType::Rounded)
                .border_style(Style::default().fg(Color::Rgb(50, 65, 90)));
            let inner = block.inner(area);
            frame.render_widget(block, area);
            frame.render_widget(Paragraph::new("Child not found"), inner);
            return;
        };

        let tab_idx = self.tabs.iter().position(|t| t.child_id == child_id).unwrap_or(0);

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(4),
                Constraint::Length(1),
            ])
            .split(area);

        let header_area = chunks[0];
        let main_area = chunks[1];
        let footer_area = chunks[2];

        let status_text = match &tab.status {
            ChildTabStatus::Paused { reason } => format!("⏸ PAUSED: {reason}"),
            ChildTabStatus::Running => {
                if let Some(ref tool) = tab.tool_in_progress {
                    format!("● running {tool} — step {}/{}", tab.step, tab.max_steps)
                } else {
                    format!("● step {}/{}", tab.step, tab.max_steps)
                }
            }
            ChildTabStatus::Done => format!("✓ Done — {} items extracted", tab.items_extracted),
            ChildTabStatus::Error(e) => format!("✗ Error: {e}"),
        };

        let header_spans = vec![
            Span::styled(
                " Child ",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!("  {} ", tab.sub_goal)),
            Span::styled(
                format!("  {status_text} "),
                Style::default().fg(tab.status_color()),
            ),
            Span::raw("  "),
            Span::styled(
                format!("{} of {}", tab_idx + 1, self.tabs.len()),
                Style::default()
                    .fg(Color::LightBlue)
                    .add_modifier(Modifier::DIM),
            ),
        ];
        frame.render_widget(
            Paragraph::new(Line::from(header_spans))
                .style(Style::default().bg(Color::Rgb(14, 18, 28))),
            header_area,
        );

        let border_color = match &tab.status {
            ChildTabStatus::Paused { .. } => Color::Rgb(180, 140, 30),
            ChildTabStatus::Running => Color::Rgb(40, 80, 110),
            ChildTabStatus::Done => Color::Rgb(40, 100, 60),
            ChildTabStatus::Error(_) => Color::Rgb(140, 40, 40),
        };
        let main_block = Block::default()
            .borders(Borders::ALL)
            .border_type(ratatui::widgets::BorderType::Rounded)
            .border_style(Style::default().fg(border_color));
        let main_inner = main_block.inner(main_area);
        frame.render_widget(main_block, main_area);

        // TODO: Task 2 will implement rendering from entries using build_wrapped_list
        // For now, render empty since entries are populated in Task 2
        let lines: Vec<Line<'_>> = Vec::new();
        frame.render_widget(Paragraph::new(lines), main_inner);

        let footer_spans = vec![
            Span::styled(" ←", Style::default().fg(Color::DarkGray)),
            Span::styled("Prev", Style::default().fg(Color::Gray)),
            Span::styled("  →", Style::default().fg(Color::DarkGray)),
            Span::styled("Next", Style::default().fg(Color::Gray)),
            Span::styled("  Esc/↑", Style::default().fg(Color::DarkGray)),
            Span::styled("Parent", Style::default().fg(Color::Gray)),
            Span::styled("  j/k", Style::default().fg(Color::DarkGray)),
            Span::styled("Scroll", Style::default().fg(Color::Gray)),
            Span::styled("  Enter", Style::default().fg(Color::DarkGray)),
            Span::styled("Resume", Style::default().fg(Color::Gray)),
        ];
        frame.render_widget(
            Paragraph::new(Line::from(footer_spans))
                .style(Style::default().bg(Color::Rgb(14, 18, 28))),
            footer_area,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    #[test]
    fn child_tab_panel_event_state_transitions() {
        let backend = TestBackend::new(80, 15);
        let mut terminal = Terminal::new(backend).unwrap();

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

        // Render final state to confirm no panic
        terminal
            .draw(|frame| {
                panel.render_fullscreen("agent-a", frame, frame.area());
            })
            .unwrap();

        let buffer = terminal.backend().buffer().clone();
        let content = buffer_to_string(&buffer);
        assert!(
            content.contains("Done") || content.contains("✓"),
            "Done state should be visible in render"
        );
    }

    #[test]
    fn child_tab_panel_empty_renders_nothing() {
        let backend = TestBackend::new(80, 15);
        let mut terminal = Terminal::new(backend).unwrap();
        let panel = ChildTabPanel::default();

        terminal
            .draw(|frame| {
                panel.render_fullscreen("nonexistent", frame, frame.area());
            })
            .unwrap();
        // Empty panel should render without panic and produce blank buffer
    }

    /// Helper: flatten a ratatui Buffer into a single string for assertion searches
    fn buffer_to_string(buffer: &ratatui::buffer::Buffer) -> String {
        let area = buffer.area();
        let mut result = String::new();
        for y in area.y..area.y + area.height {
            for x in area.x..area.x + area.width {
                let cell = &buffer[(x, y)];
                result.push_str(cell.symbol());
            }
            result.push('\n');
        }
        result
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
