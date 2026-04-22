use std::collections::HashSet;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Direction, Layout, Offset, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use api::provider::ModelInfo;
use api::provider::ProviderRegistry;

use crate::display_width::{prefix_display_width, text_display_width};
use crate::tui::grouped_model_list::{GroupedModelListState, ModelEntry, ProviderGroup, RowKind};
use crate::tui::modal::{draw_modal_frame, should_passthrough_key, Modal, ModalAction};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogSource {
    Live,
    BuiltinWhileLoading,
    BuiltinFallback,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelModalOutcome {
    None,
    SwitchModel {
        model_id: String,
    },
    AuthRequired {
        provider_id: String,
        model_id: String,
    },
}

pub struct ModelModal {
    list_state: GroupedModelListState,
    configured_providers: HashSet<String>,
    outcome: ModelModalOutcome,
    scroll_offset: std::cell::Cell<usize>,
    catalog_source: CatalogSource,
}

impl ModelModal {
    pub fn new(
        registry: &ProviderRegistry,
        current_model_id: &str,
        catalog_models: Vec<ModelInfo>,
        catalog_source: CatalogSource,
    ) -> Self {
        let mut groups_map: std::collections::HashMap<String, Vec<ModelEntry>> =
            std::collections::HashMap::new();
        let mut provider_names: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();

        for model in catalog_models {
            let provider_id = model.provider_id.clone();
            let entry = ModelEntry {
                id: model.id.clone(),
                display_name: model.display_name.clone(),
            };
            groups_map
                .entry(provider_id.clone())
                .or_default()
                .push(entry);

            provider_names
                .entry(provider_id.clone())
                .or_insert_with(|| {
                    api::builtin_presets()
                        .iter()
                        .find(|p| p.id == provider_id)
                        .map_or_else(|| provider_id.clone(), |p| p.display_name.to_string())
                });
        }

        let mut groups: Vec<ProviderGroup> = groups_map
            .into_iter()
            .map(|(provider_id, mut models)| {
                models.sort_by(|a, b| a.display_name.cmp(&b.display_name));
                let provider_name = provider_names
                    .get(&provider_id)
                    .cloned()
                    .unwrap_or_else(|| provider_id.clone());
                ProviderGroup {
                    provider_id,
                    provider_name,
                    models,
                }
            })
            .collect();

        let order = [
            api::ProviderCategory::Popular,
            api::ProviderCategory::OssHosting,
            api::ProviderCategory::Specialized,
            api::ProviderCategory::Enterprise,
            api::ProviderCategory::Gateway,
            api::ProviderCategory::Other,
        ];

        let presets = api::builtin_presets();

        groups.sort_by(|a, b| {
            let cat_a = presets
                .iter()
                .find(|p| p.id == a.provider_id)
                .map_or(api::ProviderCategory::Other, |p| p.category);
            let cat_b = presets
                .iter()
                .find(|p| p.id == b.provider_id)
                .map_or(api::ProviderCategory::Other, |p| p.category);

            let pos_a = order
                .iter()
                .position(|&c| c == cat_a)
                .unwrap_or(order.len());
            let pos_b = order
                .iter()
                .position(|&c| c == cat_b)
                .unwrap_or(order.len());

            if pos_a == pos_b {
                a.provider_name.cmp(&b.provider_name)
            } else {
                pos_a.cmp(&pos_b)
            }
        });

        let configured_providers: HashSet<String> =
            registry.configured_providers().iter().cloned().collect();

        let list_state = GroupedModelListState::new(groups, Some(current_model_id.to_string()));

        Self {
            list_state,
            configured_providers,
            outcome: ModelModalOutcome::None,
            scroll_offset: std::cell::Cell::new(0),
            catalog_source,
        }
    }

    fn catalog_source_tag(source: CatalogSource) -> &'static str {
        match source {
            CatalogSource::Live => " [live]",
            CatalogSource::BuiltinWhileLoading => " [builtin; live loading]",
            CatalogSource::BuiltinFallback => " [builtin fallback]",
        }
    }

    #[allow(clippy::unused_self)]
    pub fn supports_vertical_wheel(&self) -> bool {
        true
    }

    pub fn handle_vertical_wheel(&mut self, down: bool) {
        if down {
            self.list_state.handle_down();
        } else {
            self.list_state.handle_up();
        }
    }

    pub fn outcome(&self) -> &ModelModalOutcome {
        &self.outcome
    }

    fn visible_rows(&self) -> (Vec<RowKind<'_>>, usize, usize) {
        let filtered = self.list_state.filtered_groups();
        let mut rows = Vec::new();
        let mut selectable_count = 0usize;
        let mut selected_visual_row = 0usize;

        for group in filtered {
            rows.push(RowKind::Header {
                provider_name: group.provider_name,
                provider_id: group.provider_id,
            });

            for model in group.models {
                let is_selected = selectable_count == self.list_state.selected_idx;
                if is_selected {
                    selected_visual_row = rows.len();
                }
                rows.push(RowKind::Model {
                    entry: model,
                    provider_id: group.provider_id,
                    is_selected,
                    is_current: self.list_state.is_current_model(&model.id),
                });
                selectable_count += 1;
            }
        }

        (rows, selected_visual_row, selectable_count)
    }
}

impl Modal for ModelModal {
    fn title(&self) -> &'static str {
        "Select Model"
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
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(0),
                Constraint::Length(1),
                Constraint::Length(1),
            ])
            .split(inner);

        let filter_area = sections[1];
        let separator_area = sections[2];
        let list_area = sections[3];
        let hint_area = sections[5];

        let filter_text = if self.list_state.filter.is_empty() {
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
                Span::styled(
                    self.list_state.filter.clone(),
                    Style::default().fg(Color::White),
                ),
            ])
        };
        frame.render_widget(Paragraph::new(filter_text), filter_area);

        if self.list_state.filter.is_empty() {
            let cursor_x = filter_area
                .x
                .saturating_add(u16::try_from(text_display_width("🔍 ")).unwrap_or(u16::MAX))
                .min(filter_area.right().saturating_sub(1));
            frame.set_cursor_position((cursor_x, filter_area.y));
        } else {
            let cursor_col = text_display_width("🔍 ")
                + prefix_display_width(&self.list_state.filter, self.list_state.filter_cursor);
            let cursor_x = filter_area
                .x
                .saturating_add(u16::try_from(cursor_col).unwrap_or(u16::MAX))
                .min(filter_area.right().saturating_sub(1));
            frame.set_cursor_position((cursor_x, filter_area.y));
        }

        let sep_str = "─".repeat(usize::from(separator_area.width));
        frame.render_widget(
            Paragraph::new(sep_str).style(Style::default().fg(Color::DarkGray)),
            separator_area,
        );

        let visible_rows = usize::from(list_area.height);
        if visible_rows > 0 {
            let (rows, selected_visual_row, total_selectable) = self.visible_rows();

            let mut scroll_offset = self.scroll_offset.get();
            if selected_visual_row < scroll_offset {
                scroll_offset = selected_visual_row;
            }
            if selected_visual_row >= scroll_offset + visible_rows {
                scroll_offset = selected_visual_row.saturating_sub(visible_rows - 1);
            }
            self.scroll_offset.set(scroll_offset);

            if total_selectable == 0 {
                let no_matches = Paragraph::new("No matches")
                    .style(
                        Style::default()
                            .fg(Color::Rgb(130, 136, 145))
                            .add_modifier(Modifier::DIM),
                    )
                    .alignment(ratatui::layout::Alignment::Center);
                frame.render_widget(no_matches, list_area);
            } else {
                for (i, row) in rows
                    .iter()
                    .skip(scroll_offset)
                    .take(visible_rows)
                    .enumerate()
                {
                    let row_area = list_area.offset(Offset { x: 0, y: i as i32 });
                    if row_area.y >= list_area.bottom() {
                        break;
                    }

                    let mut row_rect = row_area;
                    row_rect.height = 1;

                    match *row {
                        RowKind::Header {
                            provider_name,
                            provider_id,
                        } => {
                            let mut spans = vec![Span::styled(
                                provider_name.to_string(),
                                Style::default()
                                    .fg(Color::Rgb(130, 136, 145))
                                    .add_modifier(Modifier::BOLD),
                            )];
                            if !self.configured_providers.contains(provider_id) {
                                spans.push(Span::styled(
                                    " (not configured)",
                                    Style::default()
                                        .fg(Color::Rgb(130, 136, 145))
                                        .add_modifier(Modifier::DIM | Modifier::ITALIC),
                                ));
                            }
                            frame.render_widget(Paragraph::new(Line::from(spans)), row_rect);
                        }
                        RowKind::Model {
                            entry,
                            provider_id,
                            is_selected,
                            is_current,
                        } => {
                            let prefix = if is_current { "✓ " } else { "  " };
                            let text = if entry.display_name.is_empty() {
                                &entry.id
                            } else {
                                &entry.display_name
                            };
                            let display_text = format!("  {prefix}{text}");

                            let style = if is_selected {
                                Style::default().bg(Color::White).fg(Color::Black)
                            } else if !self.configured_providers.contains(provider_id) {
                                Style::default().fg(Color::Rgb(160, 160, 160))
                            } else {
                                Style::default().fg(Color::White)
                            };

                            frame
                                .render_widget(Paragraph::new(display_text).style(style), row_rect);
                        }
                    }
                }
            }
        }

        let hint_style = Style::default()
            .fg(Color::Rgb(130, 136, 145))
            .add_modifier(Modifier::DIM);
        let source_tag = Self::catalog_source_tag(self.catalog_source);
        let hint_text =
            format!("↑↓ Navigate  Enter Select  Esc Cancel  Type to filter{source_tag}");
        frame.render_widget(Paragraph::new(hint_text).style(hint_style), hint_area);
    }

    fn handle_key(&mut self, key: KeyEvent) -> ModalAction {
        if should_passthrough_key(&key) {
            return ModalAction::Unhandled;
        }

        match key.code {
            KeyCode::Esc => {
                if self.list_state.filter.is_empty() {
                    self.outcome = ModelModalOutcome::None;
                    ModalAction::Dismiss
                } else {
                    self.list_state.filter.clear();
                    self.list_state.filter_cursor = 0;
                    self.list_state.selected_idx = 0;
                    ModalAction::Consumed
                }
            }
            KeyCode::Enter => {
                if let Some((provider_id, model_id)) = self.list_state.selected_model() {
                    let full_model_id = format!("{provider_id}/{model_id}");
                    if self.configured_providers.contains(provider_id) {
                        self.outcome = ModelModalOutcome::SwitchModel {
                            model_id: full_model_id,
                        };
                    } else {
                        self.outcome = ModelModalOutcome::AuthRequired {
                            provider_id: provider_id.to_string(),
                            model_id: full_model_id,
                        };
                    }
                    ModalAction::Dismiss
                } else {
                    ModalAction::Consumed
                }
            }
            KeyCode::Up => {
                self.list_state.handle_up();
                ModalAction::Consumed
            }
            KeyCode::Down => {
                self.list_state.handle_down();
                ModalAction::Consumed
            }
            KeyCode::Left => {
                self.list_state.handle_left();
                ModalAction::Consumed
            }
            KeyCode::Right => {
                self.list_state.handle_right();
                ModalAction::Consumed
            }
            KeyCode::Home => {
                self.list_state.move_cursor_home();
                ModalAction::Consumed
            }
            KeyCode::End => {
                self.list_state.move_cursor_end();
                ModalAction::Consumed
            }
            KeyCode::Char(c) => {
                self.list_state.handle_char(c);
                ModalAction::Consumed
            }
            KeyCode::Backspace => {
                self.list_state.handle_backspace();
                ModalAction::Consumed
            }
            KeyCode::Delete => {
                self.list_state.handle_delete();
                ModalAction::Consumed
            }
            _ => ModalAction::Consumed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CatalogSource, ModelModal};
    use crate::tui::modal::{Modal, ModalAction};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    #[test]
    fn catalog_source_tags_are_explicit() {
        assert_eq!(
            ModelModal::catalog_source_tag(CatalogSource::Live),
            " [live]"
        );
        assert_eq!(
            ModelModal::catalog_source_tag(CatalogSource::BuiltinWhileLoading),
            " [builtin; live loading]"
        );
        assert_eq!(
            ModelModal::catalog_source_tag(CatalogSource::BuiltinFallback),
            " [builtin fallback]"
        );
    }

    #[test]
    fn control_shortcuts_passthrough_modal() {
        let registry = api::provider::ProviderRegistry::from_credentials(
            &api::load_credentials().unwrap_or_default(),
        );
        let mut modal =
            ModelModal::new(&registry, "openai/gpt-4o", Vec::new(), CatalogSource::Live);

        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(modal.handle_key(ctrl_c), ModalAction::Unhandled);
    }
}
