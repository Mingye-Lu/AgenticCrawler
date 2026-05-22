use std::cell::Cell;
use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout, Offset, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::display_width::{prefix_display_width, text_display_width};
use crate::tui::modal::{draw_modal_frame, should_passthrough_key, Modal, ModalAction};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionModalEntry {
    pub id: String,
    pub path: PathBuf,
    pub title: Option<String>,
    pub modified_epoch_secs: u64,
    pub message_count: usize,
    pub is_current: bool,
}

impl SessionModalEntry {
    fn label(&self) -> &str {
        self.title.as_deref().unwrap_or(&self.id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum SessionModalOutcome {
    #[default]
    None,
    Switch {
        id: String,
        path: PathBuf,
    },
    Delete {
        id: String,
        path: PathBuf,
        is_current: bool,
    },
    Rename {
        id: String,
        path: PathBuf,
        title: String,
    },
}

#[derive(Debug, Clone)]
struct RenameState {
    id: String,
    path: PathBuf,
    buffer_field: crate::tui::input_field::InputField,
}

pub struct SessionModal {
    entries: Vec<SessionModalEntry>,
    filter_field: crate::tui::input_field::InputField,
    selected_idx: usize,
    pending_delete: Option<String>,
    rename_mode: Option<RenameState>,
    scroll_offset: Cell<usize>,
    outcome: SessionModalOutcome,
}

impl SessionModal {
    pub fn new(mut entries: Vec<SessionModalEntry>) -> Self {
        entries.sort_by_key(|e| std::cmp::Reverse(e.modified_epoch_secs));
        Self {
            entries,
            filter_field: crate::tui::input_field::InputField::new(),
            selected_idx: 0,
            pending_delete: None,
            rename_mode: None,
            scroll_offset: Cell::new(0),
            outcome: SessionModalOutcome::None,
        }
    }

    pub fn set_entries(&mut self, mut entries: Vec<SessionModalEntry>) {
        entries.sort_by_key(|e| std::cmp::Reverse(e.modified_epoch_secs));
        self.entries = entries;
        let len = self.filtered_indices().len();
        if self.selected_idx >= len {
            self.selected_idx = len.saturating_sub(1);
        }
        self.pending_delete = None;
    }

    pub fn take_outcome(&mut self) -> SessionModalOutcome {
        std::mem::take(&mut self.outcome)
    }

    #[allow(clippy::unused_self)]
    pub fn supports_vertical_wheel(&self) -> bool {
        true
    }

    pub fn handle_vertical_wheel(&mut self, down: bool) {
        if self.rename_mode.is_some() {
            return;
        }
        if down {
            self.move_down();
        } else {
            self.move_up();
        }
    }

    fn filtered_indices(&self) -> Vec<usize> {
        let filter = self.filter_field.text();
        if filter.is_empty() {
            return (0..self.entries.len()).collect();
        }
        let needle = self.filter_field.text().to_lowercase();
        self.entries
            .iter()
            .enumerate()
            .filter(|(_, e)| {
                e.label().to_lowercase().contains(&needle) || e.id.to_lowercase().contains(&needle)
            })
            .map(|(i, _)| i)
            .collect()
    }

    fn selected_entry(&self) -> Option<&SessionModalEntry> {
        let visible = self.filtered_indices();
        visible.get(self.selected_idx).map(|&i| &self.entries[i])
    }

    fn move_up(&mut self) {
        self.pending_delete = None;
        self.selected_idx = self.selected_idx.saturating_sub(1);
    }

    fn move_down(&mut self) {
        self.pending_delete = None;
        let max = self.filtered_indices().len().saturating_sub(1);
        self.selected_idx = self.selected_idx.saturating_add(1).min(max);
    }

    fn move_page_up(&mut self, page: usize) {
        self.pending_delete = None;
        self.selected_idx = self.selected_idx.saturating_sub(page);
    }

    fn move_page_down(&mut self, page: usize) {
        self.pending_delete = None;
        let max = self.filtered_indices().len().saturating_sub(1);
        self.selected_idx = self.selected_idx.saturating_add(page).min(max);
    }

    fn filter_handle_char(&mut self, c: char) {
        self.filter_field.insert_char(c);
        self.selected_idx = 0;
    }

    fn filter_backspace(&mut self) {
        self.filter_field.backspace();
        self.selected_idx = 0;
    }

    fn filter_delete(&mut self) {
        self.filter_field.delete();
        self.selected_idx = 0;
    }

    fn enter_rename_mode(&mut self) {
        let Some(entry) = self.selected_entry() else {
            return;
        };
        let buffer = entry.title.clone().unwrap_or_default();
        self.rename_mode = Some(RenameState {
            id: entry.id.clone(),
            path: entry.path.clone(),
            buffer_field: crate::tui::input_field::InputField::new().with_text(buffer),
        });
        self.pending_delete = None;
    }

    fn rename_handle_char(state: &mut RenameState, c: char) {
        state.buffer_field.insert_char(c);
    }

    fn rename_backspace(state: &mut RenameState) {
        state.buffer_field.backspace();
    }

    fn rename_delete(state: &mut RenameState) {
        state.buffer_field.delete();
    }

    fn handle_rename_key(&mut self, key: KeyEvent) -> ModalAction {
        if should_passthrough_key(&key) {
            return ModalAction::Unhandled;
        }
        let Some(state) = self.rename_mode.as_mut() else {
            return ModalAction::Consumed;
        };
        match key.code {
            KeyCode::Esc => {
                self.rename_mode = None;
                ModalAction::Consumed
            }
            KeyCode::Enter => {
                let title = state.buffer_field.text().trim().to_string();
                let id = state.id.clone();
                let path = state.path.clone();
                self.rename_mode = None;
                if let Some(entry) = self.entries.iter_mut().find(|e| e.id == id) {
                    entry.title = if title.is_empty() {
                        None
                    } else {
                        Some(title.clone())
                    };
                }
                self.outcome = SessionModalOutcome::Rename { id, path, title };
                ModalAction::Consumed
            }
            KeyCode::Left => {
                state.buffer_field.move_cursor_left();
                ModalAction::Consumed
            }
            KeyCode::Right => {
                state.buffer_field.move_cursor_right();
                ModalAction::Consumed
            }
            KeyCode::Home => {
                state.buffer_field.move_cursor_home();
                ModalAction::Consumed
            }
            KeyCode::End => {
                state.buffer_field.move_cursor_end();
                ModalAction::Consumed
            }
            KeyCode::Backspace => {
                Self::rename_backspace(state);
                ModalAction::Consumed
            }
            KeyCode::Delete => {
                Self::rename_delete(state);
                ModalAction::Consumed
            }
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                Self::rename_handle_char(state, c);
                ModalAction::Consumed
            }
            _ => ModalAction::Consumed,
        }
    }
}

impl Modal for SessionModal {
    fn title(&self) -> &'static str {
        "Sessions"
    }

    #[allow(
        clippy::too_many_lines,
        clippy::cast_possible_truncation,
        clippy::cast_possible_wrap
    )]
    fn draw(&self, frame: &mut Frame<'_>, area: Rect) {
        let inner = draw_modal_frame(frame, area, self.title(), Color::Cyan);

        let sections = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // top spacer
                Constraint::Length(1), // filter / rename input
                Constraint::Length(1), // separator
                Constraint::Min(0),    // list
                Constraint::Length(1), // bottom spacer
                Constraint::Length(1), // hint
            ])
            .split(inner);

        let input_area = sections[1];
        let separator_area = sections[2];
        let list_area = sections[3];
        let hint_area = sections[5];

        // Input line: filter or rename buffer.
        if let Some(state) = self.rename_mode.as_ref() {
            let prefix = "✏️  rename: ";
            let line = Line::from(vec![
                Span::raw(prefix),
                Span::styled(
                    state.buffer_field.text().to_string(),
                    Style::default().fg(Color::White),
                ),
            ]);
            frame.render_widget(Paragraph::new(line), input_area);
            let cursor_col = text_display_width(prefix)
                + prefix_display_width(state.buffer_field.text(), state.buffer_field.cursor());
            let cursor_x = input_area
                .x
                .saturating_add(u16::try_from(cursor_col).unwrap_or(u16::MAX))
                .min(input_area.right().saturating_sub(1));
            frame.set_cursor_position((cursor_x, input_area.y));
        } else {
            let filter = self.filter_field.text();
            let filter_cursor = self.filter_field.cursor();
            let filter_text = if filter.is_empty() {
                Line::from(vec![
                    Span::raw("🔍 "),
                    Span::styled(
                        "Type to filter...",
                        Style::default()
                            .fg(Color::Rgb(130, 136, 145))
                            .add_modifier(Modifier::DIM),
                    ),
                ])
            } else {
                Line::from(vec![
                    Span::raw("🔍 "),
                    Span::styled(filter.to_string(), Style::default().fg(Color::White)),
                ])
            };
            frame.render_widget(Paragraph::new(filter_text), input_area);
            let cursor_col = if filter.is_empty() {
                text_display_width("🔍 ")
            } else {
                text_display_width("🔍 ") + prefix_display_width(filter, filter_cursor)
            };
            let cursor_x = input_area
                .x
                .saturating_add(u16::try_from(cursor_col).unwrap_or(u16::MAX))
                .min(input_area.right().saturating_sub(1));
            frame.set_cursor_position((cursor_x, input_area.y));
        }

        // Separator.
        let sep_str = "─".repeat(usize::from(separator_area.width));
        frame.render_widget(
            Paragraph::new(sep_str).style(Style::default().fg(Color::DarkGray)),
            separator_area,
        );

        // List.
        let visible_rows = usize::from(list_area.height);
        let filtered = self.filtered_indices();
        if visible_rows > 0 {
            if filtered.is_empty() {
                let placeholder = if self.entries.is_empty() {
                    "No saved sessions yet."
                } else {
                    "No matches"
                };
                let no_matches = Paragraph::new(placeholder)
                    .style(
                        Style::default()
                            .fg(Color::Rgb(130, 136, 145))
                            .add_modifier(Modifier::DIM),
                    )
                    .alignment(ratatui::layout::Alignment::Center);
                frame.render_widget(no_matches, list_area);
            } else {
                let mut scroll_offset = self.scroll_offset.get();
                if self.selected_idx < scroll_offset {
                    scroll_offset = self.selected_idx;
                }
                if self.selected_idx >= scroll_offset + visible_rows {
                    scroll_offset = self.selected_idx.saturating_sub(visible_rows - 1);
                }
                self.scroll_offset.set(scroll_offset);

                for (i, &entry_idx) in filtered
                    .iter()
                    .skip(scroll_offset)
                    .take(visible_rows)
                    .enumerate()
                {
                    let entry = &self.entries[entry_idx];
                    let row_area = list_area.offset(Offset { x: 0, y: i as i32 });
                    if row_area.y >= list_area.bottom() {
                        break;
                    }
                    let mut row_rect = row_area;
                    row_rect.height = 1;

                    let is_selected = scroll_offset + i == self.selected_idx;
                    let is_pending_delete =
                        self.pending_delete.as_deref() == Some(entry.id.as_str()) && is_selected;
                    let marker = if entry.is_current { "✓ " } else { "  " };

                    let body = if is_pending_delete {
                        format!("  {marker}Press Ctrl+X again to confirm delete")
                    } else {
                        format!(
                            "  {marker}{label:<32} id={id:<22} msgs={msgs}",
                            label = truncate_label(entry.label(), 32),
                            id = entry.id,
                            msgs = entry.message_count,
                        )
                    };

                    let style = if is_pending_delete {
                        Style::default().bg(Color::Red).fg(Color::White)
                    } else if is_selected {
                        Style::default().bg(Color::White).fg(Color::Black)
                    } else if entry.is_current {
                        Style::default().fg(Color::Cyan)
                    } else {
                        Style::default().fg(Color::White)
                    };

                    frame.render_widget(Paragraph::new(body).style(style), row_rect);
                }
            }
        }

        // Hint.
        let hint_style = Style::default()
            .fg(Color::Rgb(130, 136, 145))
            .add_modifier(Modifier::DIM);
        let hint_text = if self.rename_mode.is_some() {
            "Enter Save  Esc Cancel".to_string()
        } else if self.pending_delete.is_some() {
            "Ctrl+X again to confirm  Any arrow to cancel  Esc Close".to_string()
        } else {
            "↑↓ Nav  Enter Switch  Ctrl+X Delete  Ctrl+R Rename  Esc Close".to_string()
        };
        frame.render_widget(Paragraph::new(hint_text).style(hint_style), hint_area);
    }

    #[allow(clippy::too_many_lines)]
    fn handle_key(&mut self, key: KeyEvent) -> ModalAction {
        if self.rename_mode.is_some() {
            return self.handle_rename_key(key);
        }
        if should_passthrough_key(&key) {
            return ModalAction::Unhandled;
        }

        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);

        match key.code {
            KeyCode::Esc => {
                if self.filter_field.is_empty() && self.pending_delete.is_none() {
                    ModalAction::Dismiss
                } else {
                    self.filter_field.clear();
                    self.selected_idx = 0;
                    self.pending_delete = None;
                    ModalAction::Consumed
                }
            }
            KeyCode::Enter => {
                if let Some(entry) = self.selected_entry() {
                    self.outcome = SessionModalOutcome::Switch {
                        id: entry.id.clone(),
                        path: entry.path.clone(),
                    };
                    ModalAction::Dismiss
                } else {
                    ModalAction::Consumed
                }
            }
            KeyCode::Up => {
                self.move_up();
                ModalAction::Consumed
            }
            KeyCode::Down => {
                self.move_down();
                ModalAction::Consumed
            }
            KeyCode::PageUp => {
                self.move_page_up(5);
                ModalAction::Consumed
            }
            KeyCode::PageDown => {
                self.move_page_down(5);
                ModalAction::Consumed
            }
            KeyCode::Left => {
                self.filter_field.move_cursor_left();
                ModalAction::Consumed
            }
            KeyCode::Right => {
                self.filter_field.move_cursor_right();
                ModalAction::Consumed
            }
            KeyCode::Home => {
                self.filter_field.move_cursor_home();
                ModalAction::Consumed
            }
            KeyCode::End => {
                self.filter_field.move_cursor_end();
                ModalAction::Consumed
            }
            KeyCode::Char('x') if ctrl => {
                let Some(entry) = self.selected_entry() else {
                    return ModalAction::Consumed;
                };
                if self.pending_delete.as_deref() == Some(entry.id.as_str()) {
                    let id = entry.id.clone();
                    let path = entry.path.clone();
                    let is_current = entry.is_current;
                    self.pending_delete = None;
                    self.outcome = SessionModalOutcome::Delete {
                        id,
                        path,
                        is_current,
                    };
                } else {
                    self.pending_delete = Some(entry.id.clone());
                }
                ModalAction::Consumed
            }
            KeyCode::Char('r') if ctrl => {
                self.enter_rename_mode();
                ModalAction::Consumed
            }
            KeyCode::Char(c) if !ctrl && !alt => {
                self.filter_handle_char(c);
                self.pending_delete = None;
                ModalAction::Consumed
            }
            KeyCode::Backspace => {
                self.filter_backspace();
                self.pending_delete = None;
                ModalAction::Consumed
            }
            KeyCode::Delete => {
                self.filter_delete();
                self.pending_delete = None;
                ModalAction::Consumed
            }
            _ => ModalAction::Consumed,
        }
    }
}

fn truncate_label(label: &str, max: usize) -> String {
    let count = label.chars().count();
    if count <= max {
        return label.to_string();
    }
    let mut out: String = label.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::{SessionModal, SessionModalEntry, SessionModalOutcome};
    use crate::tui::modal::{Modal, ModalAction};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use std::path::PathBuf;

    fn entry(id: &str, title: Option<&str>, modified: u64, is_current: bool) -> SessionModalEntry {
        SessionModalEntry {
            id: id.to_string(),
            path: PathBuf::from(format!("/tmp/{id}.json")),
            title: title.map(ToOwned::to_owned),
            modified_epoch_secs: modified,
            message_count: 0,
            is_current,
        }
    }

    fn sample() -> SessionModal {
        SessionModal::new(vec![
            entry("session-3", Some("Refactor login"), 30, false),
            entry("session-1", Some("Welcome chat"), 10, false),
            entry("session-2", None, 20, true),
        ])
    }

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
    }

    #[test]
    fn entries_sorted_newest_first() {
        let modal = sample();
        assert_eq!(modal.entries[0].id, "session-3");
        assert_eq!(modal.entries[1].id, "session-2");
        assert_eq!(modal.entries[2].id, "session-1");
    }

    #[test]
    fn filter_narrows_by_title_or_id_case_insensitive() {
        let mut modal = sample();
        for c in "REF".chars() {
            modal.handle_key(press(KeyCode::Char(c)));
        }
        let visible = modal.filtered_indices();
        assert_eq!(visible.len(), 1);
        assert_eq!(modal.entries[visible[0]].id, "session-3");

        modal.handle_key(press(KeyCode::Esc)); // clears filter
        assert_eq!(modal.filtered_indices().len(), 3);
    }

    #[test]
    fn enter_emits_switch_outcome_for_top_row() {
        let mut modal = sample();
        let action = modal.handle_key(press(KeyCode::Enter));
        assert_eq!(action, ModalAction::Dismiss);
        match modal.take_outcome() {
            SessionModalOutcome::Switch { id, .. } => assert_eq!(id, "session-3"),
            other => panic!("expected Switch, got {other:?}"),
        }
    }

    #[test]
    fn ctrl_x_requires_two_presses_to_emit_delete() {
        let mut modal = sample();
        modal.handle_key(ctrl(KeyCode::Char('x')));
        assert_eq!(modal.pending_delete.as_deref(), Some("session-3"));
        assert!(matches!(modal.outcome, SessionModalOutcome::None));

        let action = modal.handle_key(ctrl(KeyCode::Char('x')));
        assert_eq!(action, ModalAction::Consumed);
        match modal.take_outcome() {
            SessionModalOutcome::Delete { id, is_current, .. } => {
                assert_eq!(id, "session-3");
                assert!(!is_current);
            }
            other => panic!("expected Delete, got {other:?}"),
        }
    }

    #[test]
    fn any_arrow_clears_pending_delete() {
        let mut modal = sample();
        modal.handle_key(ctrl(KeyCode::Char('x')));
        assert!(modal.pending_delete.is_some());
        modal.handle_key(press(KeyCode::Down));
        assert!(modal.pending_delete.is_none());
    }

    #[test]
    fn ctrl_r_enters_rename_mode_then_enter_emits_rename() {
        let mut modal = sample();
        modal.handle_key(ctrl(KeyCode::Char('r')));
        assert!(modal.rename_mode.is_some());

        // Wipe the prepopulated title and type a new one.
        for _ in 0..20 {
            modal.handle_key(press(KeyCode::Backspace));
        }
        for c in "Hi".chars() {
            modal.handle_key(press(KeyCode::Char(c)));
        }
        let action = modal.handle_key(press(KeyCode::Enter));
        assert_eq!(action, ModalAction::Consumed);
        assert!(modal.rename_mode.is_none());

        match modal.take_outcome() {
            SessionModalOutcome::Rename { id, title, .. } => {
                assert_eq!(id, "session-3");
                assert_eq!(title, "Hi");
            }
            other => panic!("expected Rename, got {other:?}"),
        }
    }

    #[test]
    fn esc_in_rename_mode_cancels_without_outcome() {
        let mut modal = sample();
        modal.handle_key(ctrl(KeyCode::Char('r')));
        for c in "garbage".chars() {
            modal.handle_key(press(KeyCode::Char(c)));
        }
        modal.handle_key(press(KeyCode::Esc));
        assert!(modal.rename_mode.is_none());
        assert!(matches!(modal.outcome, SessionModalOutcome::None));
    }

    #[test]
    fn esc_with_empty_filter_dismisses() {
        let mut modal = sample();
        let action = modal.handle_key(press(KeyCode::Esc));
        assert_eq!(action, ModalAction::Dismiss);
    }

    #[test]
    fn ctrl_c_is_unhandled_so_global_exit_works() {
        let mut modal = sample();
        let action = modal.handle_key(ctrl(KeyCode::Char('c')));
        assert_eq!(action, ModalAction::Unhandled);
    }

    #[test]
    fn ctrl_x_with_no_filter_match_is_noop() {
        let mut modal = SessionModal::new(Vec::new());
        let action = modal.handle_key(ctrl(KeyCode::Char('x')));
        assert_eq!(action, ModalAction::Consumed);
        assert!(modal.pending_delete.is_none());
        assert!(matches!(modal.outcome, SessionModalOutcome::None));
    }
}
