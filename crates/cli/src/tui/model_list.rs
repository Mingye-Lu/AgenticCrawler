#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelInfo {
    pub id: String,
    pub display_name: Option<String>,
}

impl ModelInfo {
    pub fn display_label(&self) -> &str {
        self.display_name.as_deref().unwrap_or(&self.id)
    }
}

#[derive(Debug, Default)]
pub struct ModelListState {
    pub models: Vec<ModelInfo>,
    pub filter: String,
    pub filter_cursor: usize,
    pub selected_idx: usize,
}

impl ModelListState {
    fn filter_len(&self) -> usize {
        self.filter.chars().count()
    }

    fn cursor_byte_index(&self) -> usize {
        self.filter
            .char_indices()
            .nth(self.filter_cursor)
            .map_or(self.filter.len(), |(idx, _)| idx)
    }

    fn clamp_cursor(&mut self) {
        self.filter_cursor = self.filter_cursor.min(self.filter_len());
    }

    pub fn filtered(&self) -> Vec<&ModelInfo> {
        if self.filter.is_empty() {
            self.models.iter().collect()
        } else {
            let filter_lower = self.filter.to_lowercase();
            self.models
                .iter()
                .filter(|m| {
                    m.id.to_lowercase().contains(&filter_lower)
                        || m.display_name
                            .as_ref()
                            .is_some_and(|name| name.to_lowercase().contains(&filter_lower))
                })
                .collect()
        }
    }

    pub fn handle_char(&mut self, c: char) {
        let idx = self.cursor_byte_index();
        self.filter.insert(idx, c);
        self.filter_cursor = self.filter_cursor.saturating_add(1);
        self.selected_idx = 0;
    }

    pub fn handle_backspace(&mut self) {
        if self.filter_cursor == 0 {
            return;
        }
        let remove_char = self.filter_cursor - 1;
        if let Some((start, _)) = self.filter.char_indices().nth(remove_char) {
            let end = self
                .filter
                .char_indices()
                .nth(remove_char + 1)
                .map_or(self.filter.len(), |(idx, _)| idx);
            self.filter.replace_range(start..end, "");
            self.filter_cursor -= 1;
        }
        self.clamp_cursor();
        self.selected_idx = 0;
    }

    pub fn handle_delete(&mut self) {
        if self.filter_cursor >= self.filter_len() {
            return;
        }
        if let Some((start, _)) = self.filter.char_indices().nth(self.filter_cursor) {
            let end = self
                .filter
                .char_indices()
                .nth(self.filter_cursor + 1)
                .map_or(self.filter.len(), |(idx, _)| idx);
            self.filter.replace_range(start..end, "");
        }
        self.clamp_cursor();
        self.selected_idx = 0;
    }

    pub fn move_cursor_left(&mut self) {
        self.filter_cursor = self.filter_cursor.saturating_sub(1);
    }

    pub fn move_cursor_right(&mut self) {
        self.filter_cursor = (self.filter_cursor + 1).min(self.filter_len());
    }

    pub fn move_cursor_home(&mut self) {
        self.filter_cursor = 0;
    }

    pub fn move_cursor_end(&mut self) {
        self.filter_cursor = self.filter_len();
    }

    pub fn handle_up(&mut self) {
        self.selected_idx = self.selected_idx.saturating_sub(1);
    }

    pub fn handle_down(&mut self) {
        let max = self.filtered().len().saturating_sub(1);
        self.selected_idx = (self.selected_idx + 1).min(max);
    }

    pub fn selected_model(&self) -> Option<&ModelInfo> {
        self.filtered().get(self.selected_idx).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_state() -> ModelListState {
        ModelListState {
            models: vec![
                ModelInfo {
                    id: "claude-sonnet-4-6".to_string(),
                    display_name: None,
                },
                ModelInfo {
                    id: "claude-opus-4-6".to_string(),
                    display_name: None,
                },
                ModelInfo {
                    id: "gpt-4o".to_string(),
                    display_name: None,
                },
            ],
            filter: String::new(),
            filter_cursor: 0,
            selected_idx: 0,
        }
    }

    #[test]
    fn filter_by_substring() {
        let mut state = setup_state();
        state.filter = "sonnet".to_string();
        let filtered = state.filtered();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].id, "claude-sonnet-4-6");
    }

    #[test]
    fn empty_filter_shows_all() {
        let state = setup_state();
        let filtered = state.filtered();
        assert_eq!(filtered.len(), 3);
    }

    #[test]
    fn handle_backspace_removes_char() {
        let mut state = setup_state();
        state.handle_char('s');
        state.handle_char('o');
        state.handle_char('n');
        assert_eq!(state.filter, "son");
        assert_eq!(state.filter_cursor, 3);
        state.handle_backspace();
        assert_eq!(state.filter, "so");
        assert_eq!(state.filter_cursor, 2);
    }
}
