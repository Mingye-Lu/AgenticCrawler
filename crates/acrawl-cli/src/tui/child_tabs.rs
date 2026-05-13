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
        self.tabs.push(ChildTabState::new(child_id.to_string(), sub_goal.to_string()));
        self.active_tab = self.tabs.len() - 1;
        self.tabs.len() - 1
    }

    pub fn next_tab(&mut self) {
        if !self.tabs.is_empty() {
            self.active_tab = (self.active_tab + 1) % self.tabs.len();
        }
    }

    pub fn prev_tab(&mut self) {
        if !self.tabs.is_empty() && self.active_tab > 0 {
            self.active_tab -= 1;
        }
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
                tab.append_text(text);
            }
            crawler::ChildEventKind::ToolCallStart { name, input_summary } => {
                tab.tool_in_progress = Some(name.clone());
                tab.append_text(&format!("[tool] {name}: {input_summary}"));
            }
            crawler::ChildEventKind::ToolCallComplete { name, output_summary, is_error } => {
                tab.tool_in_progress = None;
                let prefix = if *is_error { "[error]" } else { "[done]" };
                tab.append_text(&format!("{prefix} {name}: {output_summary}"));
            }
            crawler::ChildEventKind::PauseRequested { reason } => {
                tab.status = ChildTabStatus::Paused { reason: reason.clone() };
                self.active_tab = idx;
            }
            crawler::ChildEventKind::Resumed => {
                tab.status = ChildTabStatus::Running;
            }
            crawler::ChildEventKind::Finished { success, items_extracted, error } => {
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
            .map(|t| matches!(t.status, ChildTabStatus::Paused { .. }))
            .unwrap_or(false)
    }

    pub fn active_child_id(&self) -> Option<&str> {
        self.tabs.get(self.active_tab).map(|t| t.child_id.as_str())
    }

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
            let lines: Vec<Line<'_>> =
                tab.scrollback.iter().skip(start).map(|s| Line::raw(s.clone())).collect();
            frame.render_widget(Paragraph::new(lines), inner);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        panel.apply_event("c2", "g2", &crawler::ChildEventKind::PauseRequested {
            reason: "captcha".into(),
        });
        assert_eq!(panel.active_tab, 1);
        assert!(panel.active_tab_is_paused());
    }

    #[test]
    fn two_children_pause_independently() {
        let mut panel = ChildTabPanel::default();
        panel.get_or_create_tab("c1", "g1");
        panel.get_or_create_tab("c2", "g2");
        panel.apply_event("c1", "g1", &crawler::ChildEventKind::PauseRequested { reason: "r1".into() });
        panel.apply_event("c2", "g2", &crawler::ChildEventKind::PauseRequested { reason: "r2".into() });
        panel.apply_event("c1", "g1", &crawler::ChildEventKind::Resumed);
        assert_eq!(panel.tabs[0].status, ChildTabStatus::Running);
        assert!(matches!(panel.tabs[1].status, ChildTabStatus::Paused { .. }));
    }
}
