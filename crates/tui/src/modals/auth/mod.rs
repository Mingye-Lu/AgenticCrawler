use std::sync::mpsc::Sender;
use std::thread;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph, Wrap};
use zeroize::Zeroizing;

use crate::auth::ProviderChoice;
use crate::display_width::{prefix_display_width, text_display_width};
use crate::tui::modal::{draw_modal_frame, should_passthrough_key, Modal, ModalAction};
use crate::tui::ReplTuiEvent;

#[allow(clippy::wildcard_imports)]
mod draw;
#[allow(clippy::wildcard_imports)]
mod handlers;
#[cfg(test)]
mod tests;

use self::draw::DrawState;

fn flat_preset_list() -> Vec<api::ProviderPreset> {
    use api::ProviderCategory;
    let order = [
        ProviderCategory::Popular,
        ProviderCategory::OssHosting,
        ProviderCategory::Specialized,
        ProviderCategory::Enterprise,
        ProviderCategory::Gateway,
        ProviderCategory::Other,
    ];
    let all = api::builtin_presets();
    let mut out = Vec::new();
    for cat in &order {
        for p in all.iter().filter(|p| p.category == *cat) {
            out.push(*p);
        }
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProviderKind {
    Anthropic,
    OpenAi,
    Other,
    Preset(api::ProviderPreset),
}

impl ProviderKind {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Anthropic => "Anthropic",
            Self::OpenAi => "OpenAI",
            Self::Other => "Other (OpenAI-compatible)",
            Self::Preset(p) => p.display_name,
        }
    }
}

impl From<crate::app::Provider> for ProviderKind {
    fn from(value: crate::app::Provider) -> Self {
        match value {
            crate::app::Provider::Anthropic => Self::Anthropic,
            crate::app::Provider::OpenAi => Self::OpenAi,
            crate::app::Provider::Other => Self::Other,
        }
    }
}

pub(crate) enum AuthModalStep {
    ProviderSelect {
        selected: usize,
    },
    AuthMethodSelect {
        provider: ProviderKind,
        selected: usize,
    },
    BaseUrlInput {
        provider: ProviderKind,
        input: String,
        cursor: usize,
        error: Option<String>,
    },
    ApiKeyInput {
        provider: ProviderKind,
        base_url: Option<String>,
        // Wrapped so the heap-allocated bytes are zeroed when the modal
        // transitions out of this step (or is dropped), keeping the API key
        // out of memory past the moment it's saved.
        key_buffer: Zeroizing<String>,
        cursor: usize,
        masked: bool,
        error: Option<String>,
    },
    OAuthWaiting {
        provider: ProviderKind,
        status: String,
        cancel_tx: Option<Sender<()>>,
        tick: u8,
    },
    ModelFetchLoading {
        provider: ProviderKind,
    },
    ModelSelect {
        provider: ProviderKind,
        state: crate::tui::model_list::ModelListState,
    },
    Success {
        message: String,
    },
    Error {
        message: String,
    },
}

pub(crate) struct AuthModal {
    pub(crate) step: AuthModalStep,
    pub(super) ui_tx: Sender<ReplTuiEvent>,
    pub(super) model_fetch_in_flight: bool,
}

impl AuthModal {
    pub(super) fn char_len(value: &str) -> usize {
        value.chars().count()
    }

    pub(super) fn char_to_byte(value: &str, char_idx: usize) -> usize {
        value
            .char_indices()
            .nth(char_idx)
            .map_or(value.len(), |(idx, _)| idx)
    }

    pub(super) fn insert_char_at(value: &mut String, cursor: &mut usize, ch: char) {
        let idx = Self::char_to_byte(value, *cursor);
        value.insert(idx, ch);
        *cursor = cursor.saturating_add(1);
    }

    pub(super) fn remove_prev_char(value: &mut String, cursor: &mut usize) {
        if *cursor == 0 {
            return;
        }
        let remove_char = *cursor - 1;
        let start = Self::char_to_byte(value, remove_char);
        let end = Self::char_to_byte(value, remove_char + 1);
        value.replace_range(start..end, "");
        *cursor -= 1;
    }

    pub(super) fn remove_current_char(value: &mut String, cursor: usize) {
        if cursor >= Self::char_len(value) {
            return;
        }
        let start = Self::char_to_byte(value, cursor);
        let end = Self::char_to_byte(value, cursor + 1);
        value.replace_range(start..end, "");
    }

    pub(super) fn auth_method_count(provider: ProviderKind) -> usize {
        if matches!(provider, ProviderKind::Other | ProviderKind::Preset(_)) {
            1
        } else {
            2
        }
    }

    pub(super) fn provider_select_index(provider: ProviderKind) -> usize {
        match provider {
            ProviderKind::Anthropic => 0,
            ProviderKind::OpenAi => 1,
            ProviderKind::Other => flat_preset_list()
                .iter()
                .position(|p| p.id == "other")
                .unwrap_or(0),
            ProviderKind::Preset(p) => flat_preset_list()
                .iter()
                .position(|pp| pp.id == p.id)
                .unwrap_or(0),
        }
    }

    pub(crate) fn new(ui_tx: Sender<ReplTuiEvent>, provider: Option<crate::app::Provider>) -> Self {
        Self::new_with_choice(ui_tx, provider.map(ProviderChoice::Legacy))
    }

    pub(crate) fn new_with_choice(
        ui_tx: Sender<ReplTuiEvent>,
        choice: Option<ProviderChoice>,
    ) -> Self {
        let step = if let Some(c) = choice {
            match c {
                ProviderChoice::Legacy(p) => match p {
                    crate::app::Provider::OpenAi => AuthModalStep::ApiKeyInput {
                        provider: ProviderKind::OpenAi,
                        base_url: None,
                        key_buffer: Zeroizing::new(String::new()),
                        cursor: 0,
                        masked: true,
                        error: None,
                    },
                    crate::app::Provider::Anthropic => AuthModalStep::OAuthWaiting {
                        provider: ProviderKind::Anthropic,
                        status: "Preparing OAuth flow...".to_string(),
                        cancel_tx: None,
                        tick: 0,
                    },
                    crate::app::Provider::Other => AuthModalStep::BaseUrlInput {
                        provider: ProviderKind::Other,
                        input: String::new(),
                        cursor: 0,
                        error: None,
                    },
                },
                ProviderChoice::Preset(preset) => {
                    if preset.id == "copilot" {
                        AuthModalStep::OAuthWaiting {
                            provider: ProviderKind::Preset(preset),
                            status: "Preparing device code flow...".to_string(),
                            cancel_tx: None,
                            tick: 0,
                        }
                    } else if preset.base_url.contains('{') {
                        AuthModalStep::BaseUrlInput {
                            provider: ProviderKind::Preset(preset),
                            input: preset.base_url.to_string(),
                            cursor: preset.base_url.chars().count(),
                            error: Some(
                                "Replace {placeholders} with your values, then press Enter"
                                    .to_string(),
                            ),
                        }
                    } else {
                        AuthModalStep::ApiKeyInput {
                            provider: ProviderKind::Preset(preset),
                            base_url: None,
                            key_buffer: Zeroizing::new(String::new()),
                            cursor: 0,
                            masked: true,
                            error: None,
                        }
                    }
                }
            }
        } else {
            AuthModalStep::ProviderSelect { selected: 0 }
        };

        Self {
            step,
            ui_tx,
            model_fetch_in_flight: false,
        }
    }

    #[allow(clippy::too_many_lines)]
    pub(super) fn fetch_models_for_provider(
        provider: ProviderKind,
    ) -> Result<Vec<acrawl_ui::events::PickerModelInfo>, String> {
        let store = api::credentials::load_credentials().unwrap_or_default();
        let provider_str = match provider {
            ProviderKind::Anthropic => "anthropic",
            ProviderKind::OpenAi => "openai",
            ProviderKind::Other => "other",
            ProviderKind::Preset(p) => p.id,
        };
        let config = store
            .providers
            .get(provider_str)
            .cloned()
            .unwrap_or_default();

        match provider {
            ProviderKind::Anthropic => {
                let key = config.api_key.unwrap_or_default();
                let runtime = crate::TOKIO_RUNTIME
                    .get()
                    .ok_or_else(|| "tokio runtime not initialised".to_string())?;
                runtime
                    .block_on(api::models::list_anthropic_models(&key))
                    .map(|models| {
                        models
                            .into_iter()
                            .map(|m| acrawl_ui::events::PickerModelInfo {
                                id: m.id,
                                display_name: m.display_name,
                            })
                            .collect()
                    })
                    .map_err(|e| e.to_string())
            }
            ProviderKind::OpenAi => {
                let runtime = crate::TOKIO_RUNTIME
                    .get()
                    .ok_or_else(|| "tokio runtime not initialised".to_string())?;
                if config.auth_method == "oauth" {
                    runtime
                        .block_on(api::models::list_models_dev("openai"))
                        .map(|models| {
                            models
                                .into_iter()
                                .map(|m| acrawl_ui::events::PickerModelInfo {
                                    id: m.id,
                                    display_name: None,
                                })
                                .collect()
                        })
                        .map_err(|e| e.to_string())
                } else {
                    let auth = config.api_key.and_then(|key| {
                        if key.trim().is_empty() {
                            None
                        } else {
                            Some(api::AuthSource::ApiKey(key))
                        }
                    });
                    if let Some(auth) = auth {
                        runtime
                            .block_on(api::models::list_openai_models(&auth))
                            .map(|models| {
                                models
                                    .into_iter()
                                    .map(|m| acrawl_ui::events::PickerModelInfo {
                                        id: m.id,
                                        display_name: None,
                                    })
                                    .collect()
                            })
                            .map_err(|e| e.to_string())
                    } else {
                        Ok(vec![])
                    }
                }
            }
            ProviderKind::Other => Ok(vec![]),
            ProviderKind::Preset(p) => {
                let runtime = crate::TOKIO_RUNTIME
                    .get()
                    .ok_or_else(|| "tokio runtime not initialised".to_string())?;
                let models = runtime
                    .block_on(api::models::list_models_dev(p.id))
                    .unwrap_or_default();
                if !models.is_empty() {
                    return Ok(models
                        .into_iter()
                        .map(|m| acrawl_ui::events::PickerModelInfo {
                            id: m.id,
                            display_name: None,
                        })
                        .collect());
                }
                Ok(api::provider::catalog::builtin_models()
                    .into_iter()
                    .filter(|m| m.provider_id == p.id)
                    .map(|m| acrawl_ui::events::PickerModelInfo {
                        id: m.id,
                        display_name: Some(m.display_name),
                    })
                    .collect())
            }
        }
    }

    #[allow(clippy::needless_pass_by_value)]
    pub(super) fn save_api_key(
        provider: ProviderKind,
        base_url: Option<String>,
        key: Zeroizing<String>,
    ) {
        let (provider_str, preset_base_url): (&str, Option<String>) = match provider {
            ProviderKind::Anthropic => ("anthropic", None),
            ProviderKind::OpenAi => ("openai", None),
            ProviderKind::Other => ("other", None),
            ProviderKind::Preset(p) => {
                let url = if p.base_url.is_empty() {
                    None
                } else {
                    Some(p.base_url.to_string())
                };
                (p.id, url)
            }
        };
        let mut store = crate::auth::load_credentials_or_warn();
        let mut config = store
            .providers
            .get(provider_str)
            .cloned()
            .unwrap_or_default();
        config.auth_method = match provider {
            ProviderKind::OpenAi => "openai_key".to_string(),
            ProviderKind::Anthropic | ProviderKind::Other | ProviderKind::Preset(_) => {
                "api_key".to_string()
            }
        };
        config.api_key = Some((*key).clone());
        config.base_url = base_url.or(preset_base_url);
        api::credentials::set_provider_config(&mut store, provider_str, config);
        let _ = api::credentials::save_credentials(&store);
        if let Some(cfg) = store.providers.get_mut(provider_str) {
            if let Some(saved_key) = cfg.api_key.as_mut() {
                use zeroize::Zeroize;
                saved_key.zeroize();
            }
        }
    }

    pub(super) fn save_default_model(provider: ProviderKind, model_id: &str) {
        if model_id.trim().is_empty() {
            return;
        }
        let mut store = crate::auth::load_credentials_or_warn();
        let provider_str = match provider {
            ProviderKind::Anthropic => "anthropic",
            ProviderKind::OpenAi => "openai",
            ProviderKind::Other => "other",
            ProviderKind::Preset(p) => p.id,
        };
        let prefixed_model_id = if model_id.contains('/') {
            model_id.trim().to_string()
        } else {
            format!("{provider_str}/{}", model_id.trim())
        };
        let mut config = store
            .providers
            .get(provider_str)
            .cloned()
            .unwrap_or_default();
        config.default_model = Some(prefixed_model_id);
        api::credentials::set_provider_config(&mut store, provider_str, config);
        let _ = api::credentials::save_credentials(&store);
        let _ = runtime::update_settings(|s| {
            s.model = Some(if model_id.contains('/') {
                model_id.trim().to_string()
            } else {
                format!("{provider_str}/{}", model_id.trim())
            });
        });
    }

    pub(crate) fn process_loading(&mut self) {
        let AuthModalStep::ModelFetchLoading { provider } = self.step else {
            self.model_fetch_in_flight = false;
            return;
        };
        if self.model_fetch_in_flight {
            return;
        }

        self.model_fetch_in_flight = true;
        let ui_tx = self.ui_tx.clone();
        thread::spawn(move || {
            let result = Self::fetch_models_for_provider(provider);
            let _ = ui_tx.send(ReplTuiEvent::AuthModelsLoaded(result));
        });
    }

    pub(crate) fn finish_model_loading(
        &mut self,
        result: Result<Vec<acrawl_ui::events::PickerModelInfo>, String>,
    ) {
        self.model_fetch_in_flight = false;
        let AuthModalStep::ModelFetchLoading { provider } = self.step else {
            return;
        };

        self.step = match result {
            Ok(models) => AuthModalStep::ModelSelect {
                provider,
                state: crate::tui::model_list::ModelListState {
                    models,
                    ..Default::default()
                },
            },
            Err(e) => AuthModalStep::Error {
                message: format!("Failed to fetch models: {e}"),
            },
        };
    }

    pub(crate) fn supports_vertical_wheel(&self) -> bool {
        matches!(
            self.step,
            AuthModalStep::ProviderSelect { .. }
                | AuthModalStep::AuthMethodSelect { .. }
                | AuthModalStep::ModelSelect { .. }
        )
    }

    pub(crate) fn handle_vertical_wheel(&mut self, scroll_down: bool) {
        match &mut self.step {
            AuthModalStep::ProviderSelect { selected } => {
                let total = flat_preset_list().len();
                if total == 0 {
                    return;
                }
                if scroll_down {
                    *selected = (*selected + 1).min(total - 1);
                } else {
                    *selected = selected.saturating_sub(1);
                }
            }
            AuthModalStep::AuthMethodSelect { provider, selected } => {
                let methods_len = Self::auth_method_count(*provider);
                if scroll_down {
                    *selected = (*selected + 1).min(methods_len - 1);
                } else {
                    *selected = selected.saturating_sub(1);
                }
            }
            AuthModalStep::ModelSelect { state, .. } => {
                if scroll_down {
                    state.handle_down();
                } else {
                    state.handle_up();
                }
            }
            _ => {}
        }
    }

    fn title(&self) -> &str {
        match &self.step {
            AuthModalStep::ProviderSelect { .. } => " Auth ",
            AuthModalStep::AuthMethodSelect { provider, .. }
            | AuthModalStep::ApiKeyInput { provider, .. } => match provider {
                ProviderKind::Anthropic => " Auth · Anthropic ",
                ProviderKind::OpenAi => " Auth · OpenAI ",
                ProviderKind::Other => " Auth · Other ",
                ProviderKind::Preset(p) => p.display_name,
            },
            AuthModalStep::BaseUrlInput { .. } => " Auth · Base URL ",
            AuthModalStep::OAuthWaiting { .. } => " Auth · Waiting ",
            AuthModalStep::ModelFetchLoading { .. } => " Auth · Loading Models ",
            AuthModalStep::ModelSelect { .. } => " Auth · Select Model ",
            AuthModalStep::Success { .. } => " Auth · Success ",
            AuthModalStep::Error { .. } => " Auth · Error ",
        }
    }
}

impl Modal for AuthModal {
    fn draw(&self, frame: &mut ratatui::Frame<'_>, area: Rect) {
        let DrawState {
            border_color,
            body_lines,
            footer_hint,
            cursor_pos,
            anchor_line,
        } = self.build_draw_state();

        let scroll_for = |view_h: u16| -> u16 {
            let Some(sel) = anchor_line else {
                return 0;
            };
            let vh = usize::from(view_h.max(1));
            if sel < vh {
                0
            } else {
                u16::try_from(sel.saturating_sub(vh.saturating_sub(1))).unwrap_or(u16::MAX)
            }
        };

        let inner = if area.width <= 92 {
            let block = Block::default()
                .title(self.title())
                .title_style(
                    Style::default()
                        .fg(border_color)
                        .add_modifier(ratatui::style::Modifier::BOLD),
                )
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(border_color))
                .padding(Padding::new(1, 1, 0, 0))
                .style(Style::default().bg(Color::Rgb(16, 20, 26)));
            let inner = block.inner(area);
            frame.render_widget(Clear, area);
            frame.render_widget(block, area);
            inner
        } else {
            draw_modal_frame(frame, area, self.title(), border_color)
        };

        let (body_area, hint_area) = if footer_hint.is_some() {
            let sections = ratatui::layout::Layout::default()
                .direction(ratatui::layout::Direction::Vertical)
                .constraints([
                    ratatui::layout::Constraint::Length(1),
                    ratatui::layout::Constraint::Min(0),
                    ratatui::layout::Constraint::Length(1),
                    ratatui::layout::Constraint::Length(1),
                ])
                .split(inner);
            (sections[1], Some(sections[3]))
        } else {
            let sections = ratatui::layout::Layout::default()
                .direction(ratatui::layout::Direction::Vertical)
                .constraints([
                    ratatui::layout::Constraint::Length(1),
                    ratatui::layout::Constraint::Min(0),
                    ratatui::layout::Constraint::Length(1),
                ])
                .split(inner);
            (sections[1], None)
        };

        if body_area.height > 0 {
            let body_text = Text::from(body_lines);
            let scroll_row = scroll_for(body_area.height);
            let paragraph = Paragraph::new(body_text)
                .wrap(Wrap { trim: false })
                .scroll((scroll_row, 0));
            frame.render_widget(paragraph, body_area);
            if let Some((row, col)) = cursor_pos {
                let cursor_row = row
                    .saturating_sub(1)
                    .min(body_area.height.saturating_sub(1));
                let cursor_col = col.min(body_area.width.saturating_sub(1));
                frame.set_cursor_position((
                    body_area.x.saturating_add(cursor_col),
                    body_area.y.saturating_add(cursor_row),
                ));
            }
        }

        if let (Some(area), Some(hint)) = (hint_area, footer_hint) {
            if area.height > 0 {
                frame.render_widget(Paragraph::new(hint), area);
            }
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> ModalAction {
        if should_passthrough_key(&key) {
            return ModalAction::Unhandled;
        }

        self.handle_key_by_step(key)
    }

    fn title(&self) -> &str {
        match &self.step {
            AuthModalStep::ProviderSelect { .. } => " Auth ",
            AuthModalStep::AuthMethodSelect { provider, .. }
            | AuthModalStep::ApiKeyInput { provider, .. } => match provider {
                ProviderKind::Anthropic => " Auth · Anthropic ",
                ProviderKind::OpenAi => " Auth · OpenAI ",
                ProviderKind::Other => " Auth · Other ",
                ProviderKind::Preset(p) => p.display_name,
            },
            AuthModalStep::BaseUrlInput { .. } => " Auth · Base URL ",
            AuthModalStep::OAuthWaiting { .. } => " Auth · Waiting ",
            AuthModalStep::ModelFetchLoading { .. } => " Auth · Loading Models ",
            AuthModalStep::ModelSelect { .. } => " Auth · Select Model ",
            AuthModalStep::Success { .. } => " Auth · Success ",
            AuthModalStep::Error { .. } => " Auth · Error ",
        }
    }
}
