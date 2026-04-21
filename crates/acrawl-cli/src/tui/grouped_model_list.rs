#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelEntry {
    pub id: String,
    pub display_name: String,
}

#[derive(Debug, Clone)]
pub struct ProviderGroup {
    pub provider_id: String,
    pub provider_name: String,
    pub models: Vec<ModelEntry>,
}

#[derive(Debug)]
pub struct GroupedModelListState {
    pub groups: Vec<ProviderGroup>,
    pub filter: String,
    pub filter_cursor: usize,
    pub selected_idx: usize,
    pub current_model_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct FilteredGroup<'a> {
    pub provider_name: &'a str,
    pub provider_id: &'a str,
    pub models: Vec<&'a ModelEntry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKind<'a> {
    Header {
        provider_name: &'a str,
        provider_id: &'a str,
    },
    Model {
        entry: &'a ModelEntry,
        provider_id: &'a str,
        is_selected: bool,
        is_current: bool,
    },
}

impl GroupedModelListState {
    pub fn new(groups: Vec<ProviderGroup>, current_model_id: Option<String>) -> Self {
        Self {
            groups,
            filter: String::new(),
            filter_cursor: 0,
            selected_idx: 0,
            current_model_id,
        }
    }

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

    pub fn filtered_groups(&self) -> Vec<FilteredGroup<'_>> {
        if self.filter.is_empty() {
            return self
                .groups
                .iter()
                .map(|g| FilteredGroup {
                    provider_name: &g.provider_name,
                    provider_id: &g.provider_id,
                    models: g.models.iter().collect(),
                })
                .collect();
        }

        let filter_lower = self.filter.to_lowercase();
        self.groups
            .iter()
            .filter_map(|g| {
                let matching_models: Vec<&ModelEntry> = g
                    .models
                    .iter()
                    .filter(|m| {
                        m.id.to_lowercase().contains(&filter_lower)
                            || m.display_name.to_lowercase().contains(&filter_lower)
                            || g.provider_name.to_lowercase().contains(&filter_lower)
                    })
                    .collect();

                if matching_models.is_empty() {
                    None
                } else {
                    Some(FilteredGroup {
                        provider_name: &g.provider_name,
                        provider_id: &g.provider_id,
                        models: matching_models,
                    })
                }
            })
            .collect()
    }

    pub fn total_selectable(&self) -> usize {
        let filtered = self.filtered_groups();
        filtered.iter().map(|g| g.models.len()).sum()
    }

    pub fn handle_up(&mut self) {
        self.selected_idx = self.selected_idx.saturating_sub(1);
    }

    pub fn handle_down(&mut self) {
        let max = self.total_selectable().saturating_sub(1);
        self.selected_idx = (self.selected_idx + 1).min(max);
    }

    pub fn handle_left(&mut self) {
        self.selected_idx = 0;
    }

    pub fn handle_right(&mut self) {
        self.selected_idx = self.total_selectable().saturating_sub(1);
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

    pub fn move_cursor_home(&mut self) {
        self.filter_cursor = 0;
    }

    pub fn move_cursor_end(&mut self) {
        self.filter_cursor = self.filter_len();
    }

    pub fn selected_model(&self) -> Option<(&str, &str)> {
        let filtered = self.filtered_groups();
        let mut model_count = 0;

        for group in &filtered {
            for model in &group.models {
                if model_count == self.selected_idx {
                    return Some((group.provider_id, &model.id));
                }
                model_count += 1;
            }
        }

        None
    }

    pub fn is_current_model(&self, model_id: &str) -> bool {
        self.current_model_id
            .as_ref()
            .is_some_and(|id| id == model_id)
    }

    pub fn row_at(&self, flat_row: usize) -> Option<RowKind<'_>> {
        let filtered = self.filtered_groups();
        let mut current_row = 0;

        for group in &filtered {
            if current_row == flat_row {
                return Some(RowKind::Header {
                    provider_name: group.provider_name,
                    provider_id: group.provider_id,
                });
            }
            current_row += 1;

            for model in &group.models {
                if current_row == flat_row {
                    let mut selectable_count = 0;
                    for prev_group in &filtered {
                        for prev_model in &prev_group.models {
                            if prev_group.provider_id == group.provider_id
                                && prev_model.id == model.id
                            {
                                return Some(RowKind::Model {
                                    entry: model,
                                    provider_id: group.provider_id,
                                    is_selected: selectable_count == self.selected_idx,
                                    is_current: self.is_current_model(&model.id),
                                });
                            }
                            selectable_count += 1;
                        }
                    }
                }
                current_row += 1;
            }
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_groups() -> Vec<ProviderGroup> {
        vec![
            ProviderGroup {
                provider_id: "anthropic".into(),
                provider_name: "Anthropic".into(),
                models: vec![
                    ModelEntry {
                        id: "claude-sonnet-4-5".into(),
                        display_name: "Claude Sonnet 4.5".into(),
                    },
                    ModelEntry {
                        id: "claude-opus-4-5".into(),
                        display_name: "Claude Opus 4.5".into(),
                    },
                ],
            },
            ProviderGroup {
                provider_id: "openai".into(),
                provider_name: "OpenAI".into(),
                models: vec![ModelEntry {
                    id: "gpt-4o".into(),
                    display_name: "GPT-4o".into(),
                }],
            },
        ]
    }

    #[test]
    fn filter_by_provider_name() {
        let state = GroupedModelListState::new(make_groups(), None);
        let mut filtered_state = state;
        filtered_state.filter = "anthropic".into();

        let filtered = filtered_state.filtered_groups();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].provider_id, "anthropic");
        assert_eq!(filtered[0].models.len(), 2);
    }

    #[test]
    fn filter_by_model_id() {
        let state = GroupedModelListState::new(make_groups(), None);
        let mut filtered_state = state;
        filtered_state.filter = "sonnet".into();

        let filtered = filtered_state.filtered_groups();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].models.len(), 1);
        assert_eq!(filtered[0].models[0].id, "claude-sonnet-4-5");
    }

    #[test]
    fn empty_filter_shows_all() {
        let state = GroupedModelListState::new(make_groups(), None);
        let filtered = state.filtered_groups();
        assert_eq!(filtered.len(), 2);
        assert_eq!(filtered[0].models.len(), 2);
        assert_eq!(filtered[1].models.len(), 1);
    }

    #[test]
    fn nonsense_filter_returns_zero() {
        let state = GroupedModelListState::new(make_groups(), None);
        let mut filtered_state = state;
        filtered_state.filter = "zzzzz".into();

        assert_eq!(filtered_state.total_selectable(), 0);
    }

    #[test]
    fn handle_up_clamps_at_zero() {
        let mut state = GroupedModelListState::new(make_groups(), None);
        state.selected_idx = 0;
        state.handle_up();
        assert_eq!(state.selected_idx, 0);
    }

    #[test]
    fn handle_down_clamps_at_last() {
        let mut state = GroupedModelListState::new(make_groups(), None);
        state.selected_idx = state.total_selectable() - 1;
        state.handle_down();
        assert_eq!(state.selected_idx, state.total_selectable() - 1);
    }

    #[test]
    fn handle_left_jumps_to_first() {
        let mut state = GroupedModelListState::new(make_groups(), None);
        state.selected_idx = 2;
        state.handle_left();
        assert_eq!(state.selected_idx, 0);
    }

    #[test]
    fn handle_right_jumps_to_last() {
        let mut state = GroupedModelListState::new(make_groups(), None);
        state.selected_idx = 0;
        state.handle_right();
        assert_eq!(state.selected_idx, state.total_selectable() - 1);
    }

    #[test]
    fn navigation_skips_headers() {
        let mut state = GroupedModelListState::new(make_groups(), None);
        assert_eq!(state.total_selectable(), 3);
        state.selected_idx = 0;
        state.handle_down();
        assert_eq!(state.selected_idx, 1);
    }

    #[test]
    fn selected_model_returns_correct_tuple() {
        let mut state = GroupedModelListState::new(make_groups(), None);
        state.selected_idx = 0;
        let selected = state.selected_model();
        assert_eq!(selected, Some(("anthropic", "claude-sonnet-4-5")));

        state.selected_idx = 2;
        let selected = state.selected_model();
        assert_eq!(selected, Some(("openai", "gpt-4o")));
    }

    #[test]
    fn is_current_model_identifies_active() {
        let state = GroupedModelListState::new(make_groups(), Some("claude-sonnet-4-5".into()));
        assert!(state.is_current_model("claude-sonnet-4-5"));
        assert!(!state.is_current_model("gpt-4o"));
    }

    #[test]
    fn filter_change_resets_selection() {
        let mut state = GroupedModelListState::new(make_groups(), None);
        state.selected_idx = 2;
        state.handle_char('a');
        assert_eq!(state.selected_idx, 0);
    }

    #[test]
    fn row_at_returns_correct_kinds() {
        let state = GroupedModelListState::new(make_groups(), None);
        match state.row_at(0) {
            Some(RowKind::Header {
                provider_name,
                provider_id,
            }) => {
                assert_eq!(provider_name, "Anthropic");
                assert_eq!(provider_id, "anthropic");
            }
            _ => panic!("expected header at row 0"),
        }

        match state.row_at(1) {
            Some(RowKind::Model {
                entry,
                provider_id,
                is_selected,
                is_current,
            }) => {
                assert_eq!(entry.id, "claude-sonnet-4-5");
                assert_eq!(provider_id, "anthropic");
                assert!(is_selected);
                assert!(!is_current);
            }
            _ => panic!("expected model at row 1"),
        }
    }

    #[test]
    fn empty_provider_group_omitted() {
        let state = GroupedModelListState::new(make_groups(), None);
        let mut filtered_state = state;
        filtered_state.filter = "gpt".into();

        let filtered = filtered_state.filtered_groups();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].provider_id, "openai");
    }
}
