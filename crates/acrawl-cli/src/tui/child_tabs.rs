use std::collections::VecDeque;

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Tabs};
use ratatui::Frame;

const MAX_SCROLLBACK: usize = 1000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChildTabStatus {
    Running,
    Paused { reason: String },
    Done,
    Error(String),
}

#[derive(Debug, Clone)]
pub struct ChildTabState {
    pub child_id: String,
    pub sub_goal: String,
    pub status: ChildTabStatus,
    pub scrollback: VecDeque<String>,
    pub step: usize,
    pub max_steps: usize,
    pub tool_in_progress: Option<String>,
    pub items_extracted: usize,
    pub scroll_offset: usize,
    pub follow_bottom: bool,
}

impl ChildTabState {
    pub fn new(child_id: String, sub_goal: String) -> Self {
        Self {
            child_id,
            sub_goal,
            status: ChildTabStatus::Running,
            scrollback: VecDeque::new(),
            step: 0,
            max_steps: 0,
            tool_in_progress: None,
            items_extracted: 0,
            scroll_offset: 0,
            follow_bottom: true,
        }
    }

    pub fn append_text(&mut self, text: &str) {
        for line in text.split('\n') {
            if !line.is_empty() {
                if self.scrollback.len() >= MAX_SCROLLBACK {
                    self.scrollback.pop_front();
                }
                self.scrollback.push_back(line.to_string());
            }
        }
    }

    pub fn append_text_streaming(&mut self, text: &str) {
        let mut parts = text.split('\n');
        if let Some(first) = parts.next() {
            if !first.is_empty() {
                if let Some(last_line) = self.scrollback.back_mut() {
                    last_line.push_str(first);
                } else {
                    self.scrollback.push_back(first.to_string());
                }
            }
        }
        for line in parts {
            if self.scrollback.len() >= MAX_SCROLLBACK {
                self.scrollback.pop_front();
            }
            self.scrollback.push_back(line.to_string());
        }
    }

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

#[derive(Debug, Default)]
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

    pub fn apply_event(&mut self, child_id: &str, sub_goal: &str, event: &crawler::ChildEventKind) {
        let idx = self.get_or_create_tab(child_id, sub_goal);
        let tab = &mut self.tabs[idx];
        match event {
            crawler::ChildEventKind::StepStarted { step, max_steps } => {
                tab.step = *step;
                tab.max_steps = *max_steps;
            }
            crawler::ChildEventKind::TextDelta(text) => {
                tab.append_text_streaming(text);
            }
            crawler::ChildEventKind::ToolCallStart {
                name,
                input_summary,
            } => {
                tab.tool_in_progress = Some(name.clone());
                tab.append_text(&format!("[tool] {name}: {input_summary}"));
            }
            crawler::ChildEventKind::ToolCallComplete {
                name,
                output_summary,
                is_error,
            } => {
                tab.tool_in_progress = None;
                let prefix = if *is_error { "[error]" } else { "[done]" };
                tab.append_text(&format!("{prefix} {name}: {output_summary}"));
            }
            crawler::ChildEventKind::PauseRequested { reason } => {
                tab.status = ChildTabStatus::Paused {
                    reason: reason.clone(),
                };
                self.active_tab = idx;
            }
            crawler::ChildEventKind::Resumed => {
                tab.status = ChildTabStatus::Running;
            }
            crawler::ChildEventKind::Finished {
                success,
                items_extracted,
                error,
            } => {
                tab.items_extracted = *items_extracted;
                tab.status = if *success {
                    ChildTabStatus::Done
                } else {
                    ChildTabStatus::Error(
                        error.clone().unwrap_or_else(|| "unknown error".to_string()),
                    )
                };
            }
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

    #[allow(dead_code)]
    pub fn render(&self, frame: &mut Frame<'_>, area: Rect) {
        if self.tabs.is_empty() {
            return;
        }
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(0)])
            .split(area);

        let tab_titles: Vec<Line<'_>> = self
            .tabs
            .iter()
            .map(|t| {
                Line::from(vec![
                    Span::styled(t.status_indicator(), Style::default().fg(t.status_color())),
                    Span::raw(format!(" {}", t.child_id)),
                ])
            })
            .collect();

        let tabs_widget = Tabs::new(tab_titles)
            .select(self.active_tab)
            .style(Style::default().fg(Color::DarkGray))
            .highlight_style(
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            );
        frame.render_widget(tabs_widget, chunks[0]);

        if let Some(tab) = self.tabs.get(self.active_tab) {
            let border_color = tab.status_color();
            let title = match &tab.status {
                ChildTabStatus::Paused { reason } => {
                    format!(" ⏸ PAUSED: {reason} — press Enter to resume ")
                }
                ChildTabStatus::Running => {
                    format!(" {} — step {}/{} ", tab.sub_goal, tab.step, tab.max_steps)
                }
                ChildTabStatus::Done => format!(" ✓ Done — {} items ", tab.items_extracted),
                ChildTabStatus::Error(e) => format!(" ✗ Error: {e} "),
            };
            let block = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(border_color))
                .title(title.as_str());
            let inner = block.inner(chunks[1]);
            frame.render_widget(block, chunks[1]);

            let available = usize::from(inner.height.max(1));
            let start = tab.scrollback.len().saturating_sub(available);
            let lines: Vec<Line<'_>> = tab
                .scrollback
                .iter()
                .skip(start)
                .map(|s| Line::raw(s.clone()))
                .collect();
            frame.render_widget(Paragraph::new(lines), inner);
        }
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

        let available = usize::from(main_inner.height.max(1));
        let start = if tab.follow_bottom {
            tab.scrollback.len().saturating_sub(available)
        } else {
            tab.scroll_offset.min(tab.scrollback.len().saturating_sub(available))
        };
        let lines: Vec<Line<'_>> = tab
            .scrollback
            .iter()
            .skip(start)
            .take(available)
            .map(|s| Line::raw(s.clone()))
            .collect();
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
    fn child_tab_panel_renders_three_states() {
        let backend = TestBackend::new(80, 15);
        let mut terminal = Terminal::new(backend).unwrap();

        let mut panel = ChildTabPanel::default();
        // Child 1: running
        panel.get_or_create_tab("child-1", "scrape books");
        panel.apply_event(
            "child-1",
            "scrape books",
            &crawler::ChildEventKind::StepStarted {
                step: 3,
                max_steps: 10,
            },
        );
        // Child 2: paused
        panel.get_or_create_tab("child-2", "scrape prices");
        panel.apply_event(
            "child-2",
            "scrape prices",
            &crawler::ChildEventKind::PauseRequested {
                reason: "captcha detected".into(),
            },
        );
        // Child 3: done
        panel.get_or_create_tab("child-3", "scrape reviews");
        panel.apply_event(
            "child-3",
            "scrape reviews",
            &crawler::ChildEventKind::Finished {
                success: true,
                items_extracted: 42,
                error: None,
            },
        );

        // Switch to child-2 (paused) since that's what we want to inspect
        panel.active_tab = 1;

        terminal
            .draw(|frame| {
                panel.render(frame, frame.area());
            })
            .unwrap();

        let buffer = terminal.backend().buffer().clone();
        let content = buffer_to_string(&buffer);

        // Assert tab bar contains child IDs
        assert!(
            content.contains("child-1"),
            "Buffer should contain child-1 tab"
        );
        assert!(
            content.contains("child-2"),
            "Buffer should contain child-2 tab"
        );
        assert!(
            content.contains("child-3"),
            "Buffer should contain child-3 tab"
        );

        // Assert paused child shows pause message
        assert!(
            content.contains("PAUSED") || content.contains("captcha"),
            "Paused tab should show pause reason or PAUSED label"
        );
    }

    #[test]
    fn child_tab_panel_renders_without_panic() {
        let backend = TestBackend::new(80, 15);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut panel = ChildTabPanel::default();
        panel.get_or_create_tab("child-1", "scrape books");
        panel.get_or_create_tab("child-2", "scrape prices");
        panel.apply_event(
            "child-2",
            "scrape prices",
            &crawler::ChildEventKind::PauseRequested {
                reason: "captcha".into(),
            },
        );
        terminal
            .draw(|frame| {
                panel.render(frame, frame.area());
            })
            .unwrap();
        // If we got here without panic, rendering works
    }

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
                panel.render(frame, frame.area());
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
                panel.render(frame, frame.area());
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
    fn bounded_scrollback_does_not_exceed_max() {
        let mut tab = ChildTabState::new("child-1".into(), "goal".into());
        for i in 0..1500 {
            tab.append_text(&format!("line {i}"));
        }
        assert_eq!(tab.scrollback.len(), MAX_SCROLLBACK);
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
