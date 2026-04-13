use std::sync::mpsc::Sender;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph, Wrap};

use crate::tui::modal::{draw_modal_frame, Modal, ModalAction};
use crate::tui::ReplTuiEvent;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProviderKind {
    Anthropic,
    OpenAi,
    Other,
}

impl ProviderKind {
    fn label(self) -> &'static str {
        match self {
            Self::Anthropic => "Anthropic",
            Self::OpenAi => "OpenAI",
            Self::Other => "Other (OpenAI-compatible)",
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
        input: String,
        cursor: usize,
        error: Option<String>,
    },
    ApiKeyInput {
        provider: ProviderKind,
        base_url: Option<String>,
        key_buffer: String,
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
}

impl AuthModal {
    fn char_len(value: &str) -> usize {
        value.chars().count()
    }

    fn char_to_byte(value: &str, char_idx: usize) -> usize {
        value
            .char_indices()
            .nth(char_idx)
            .map_or(value.len(), |(idx, _)| idx)
    }

    fn insert_char_at(value: &mut String, cursor: &mut usize, ch: char) {
        let idx = Self::char_to_byte(value, *cursor);
        value.insert(idx, ch);
        *cursor = cursor.saturating_add(1);
    }

    fn remove_prev_char(value: &mut String, cursor: &mut usize) {
        if *cursor == 0 {
            return;
        }
        let remove_char = *cursor - 1;
        let start = Self::char_to_byte(value, remove_char);
        let end = Self::char_to_byte(value, remove_char + 1);
        value.replace_range(start..end, "");
        *cursor -= 1;
    }

    fn remove_current_char(value: &mut String, cursor: usize) {
        if cursor >= Self::char_len(value) {
            return;
        }
        let start = Self::char_to_byte(value, cursor);
        let end = Self::char_to_byte(value, cursor + 1);
        value.replace_range(start..end, "");
    }

    pub(crate) fn new(
        _ui_tx: Sender<ReplTuiEvent>,
        provider: Option<crate::app::Provider>,
    ) -> Self {
        let step = if let Some(p) = provider {
            match p {
                crate::app::Provider::OpenAi => AuthModalStep::ApiKeyInput {
                    provider: ProviderKind::OpenAi,
                    base_url: None,
                    key_buffer: String::new(),
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
                    input: String::new(),
                    cursor: 0,
                    error: None,
                },
            }
        } else {
            AuthModalStep::ProviderSelect { selected: 0 }
        };

        Self { step }
    }

    pub(crate) fn new_model_only(
        _ui_tx: Sender<ReplTuiEvent>,
        provider: crate::app::Provider,
    ) -> Self {
        let provider: ProviderKind = provider.into();
        Self {
            step: AuthModalStep::ModelFetchLoading { provider },
        }
    }

    fn save_api_key(provider: ProviderKind, base_url: Option<String>, key: String) {
        let mut store = api::credentials::load_credentials().unwrap_or_default();
        let provider_str = match provider {
            ProviderKind::Anthropic => "anthropic",
            ProviderKind::OpenAi => "openai",
            ProviderKind::Other => "other",
        };
        let mut config = store
            .providers
            .get(provider_str)
            .cloned()
            .unwrap_or_default();
        config.auth_method = match provider {
            ProviderKind::OpenAi => "openai_key".to_string(),
            ProviderKind::Anthropic | ProviderKind::Other => "api_key".to_string(),
        };
        config.api_key = Some(key);
        if let Some(url) = base_url {
            config.base_url = Some(url);
        }
        store.active_provider = Some(provider_str.to_string());
        api::credentials::set_provider_config(&mut store, provider_str, config);
        let _ = api::credentials::save_credentials(&store);
    }

    fn save_default_model(provider: ProviderKind, model_id: &str) {
        if model_id.trim().is_empty() {
            return;
        }
        let mut store = api::credentials::load_credentials().unwrap_or_default();
        let provider_str = match provider {
            ProviderKind::Anthropic => "anthropic",
            ProviderKind::OpenAi => "openai",
            ProviderKind::Other => "other",
        };
        let mut config = store
            .providers
            .get(provider_str)
            .cloned()
            .unwrap_or_default();
        let registry = api::provider::ProviderRegistry::from_credentials(&store);
        let canonical_model = registry.resolve_alias(model_id.trim());
        config.default_model = Some(canonical_model.to_string());
        store.active_provider = Some(provider_str.to_string());
        api::credentials::set_provider_config(&mut store, provider_str, config);
        let _ = api::credentials::save_credentials(&store);
    }

    pub(crate) fn process_loading(&mut self) {
        if let AuthModalStep::ModelFetchLoading { provider } = &self.step {
            let provider_copy = *provider;

            let store = api::credentials::load_credentials().unwrap_or_default();
            let provider_str = match provider_copy {
                ProviderKind::Anthropic => "anthropic",
                ProviderKind::OpenAi => "openai",
                ProviderKind::Other => "other",
            };
            let config = store
                .providers
                .get(provider_str)
                .cloned()
                .unwrap_or_default();

            let models_result = match provider_copy {
                ProviderKind::Anthropic => {
                    let key = config.api_key.unwrap_or_default();
                    tokio::runtime::Runtime::new()
                        .unwrap()
                        .block_on(api::models::list_anthropic_models(&key))
                        .map(|models| {
                            models
                                .into_iter()
                                .map(|m| crate::tui::model_list::ModelInfo {
                                    id: m.id,
                                    display_name: m.display_name,
                                })
                                .collect()
                        })
                }
                ProviderKind::OpenAi => {
                    if config.auth_method == "oauth" {
                        tokio::runtime::Runtime::new()
                            .unwrap()
                            .block_on(api::models::list_models_dev("openai"))
                            .map(|models| {
                                models
                                    .into_iter()
                                    .map(|m| crate::tui::model_list::ModelInfo {
                                        id: m.id,
                                        display_name: None,
                                    })
                                    .collect()
                            })
                    } else {
                        let auth = config.api_key.and_then(|key| {
                            if key.trim().is_empty() {
                                None
                            } else {
                                Some(api::AuthSource::ApiKey(key))
                            }
                        });
                        if let Some(auth) = auth {
                            tokio::runtime::Runtime::new()
                                .unwrap()
                                .block_on(api::models::list_openai_models(&auth))
                                .map(|models| {
                                    models
                                        .into_iter()
                                        .map(|m| crate::tui::model_list::ModelInfo {
                                            id: m.id,
                                            display_name: None,
                                        })
                                        .collect()
                                })
                        } else {
                            Ok(vec![])
                        }
                    }
                }
                ProviderKind::Other => Ok(vec![]),
            };

            match models_result {
                Ok(models) => {
                    self.step = AuthModalStep::ModelSelect {
                        provider: provider_copy,
                        state: crate::tui::model_list::ModelListState {
                            models,
                            ..Default::default()
                        },
                    };
                }
                Err(e) => {
                    self.step = AuthModalStep::Error {
                        message: format!("Failed to fetch models: {e}"),
                    };
                }
            }
        }
    }
}

impl Modal for AuthModal {
    #[allow(clippy::too_many_lines)]
    fn draw(&self, frame: &mut ratatui::Frame<'_>, area: Rect) {
        let (border_color, text, cursor_pos) = match &self.step {
            AuthModalStep::ProviderSelect { selected } => {
                let providers = [
                    ProviderKind::Anthropic,
                    ProviderKind::OpenAi,
                    ProviderKind::Other,
                ];
                let mut lines = providers
                    .iter()
                    .enumerate()
                    .map(|(index, p)| {
                        let cursor = if index == *selected { '>' } else { ' ' };
                        Line::from(format!("  {cursor} {}", p.label()))
                    })
                    .collect::<Vec<_>>();
                lines.push(Line::default());
                lines.push(Line::from("Up/Down navigate  Enter select  Esc cancel"));
                (Color::Cyan, Text::from(lines), None)
            }
            AuthModalStep::AuthMethodSelect { provider, selected } => {
                let methods = match provider {
                    ProviderKind::Anthropic => vec!["API Key", "OAuth"],
                    ProviderKind::OpenAi => vec!["API Key", "OAuth (Codex)"],
                    ProviderKind::Other => vec!["API Key"],
                };
                let mut lines = vec![
                    Line::from(format!("Select auth method for {}:", provider.label())),
                    Line::default(),
                ];
                for (index, method) in methods.iter().enumerate() {
                    let cursor = if index == *selected { '>' } else { ' ' };
                    lines.push(Line::from(format!("  {cursor} {method}")));
                }
                lines.push(Line::default());
                lines.push(Line::from("Up/Down navigate  Enter select  Esc back"));
                (Color::Cyan, Text::from(lines), None)
            }
            AuthModalStep::BaseUrlInput {
                input,
                cursor,
                error,
            } => {
                let mut lines = vec![
                    Line::from("Enter base URL for Other provider:"),
                    Line::default(),
                    Line::from(format!("  > {input}")),
                    Line::default(),
                ];
                if let Some(message) = error {
                    lines.push(Line::from(Span::styled(
                        message.clone(),
                        Style::default().fg(Color::Red),
                    )));
                    lines.push(Line::default());
                }
                lines.push(Line::from("←/→ move  Enter confirm  Esc back"));
                (
                    Color::Yellow,
                    Text::from(lines),
                    Some((
                        3u16,
                        4u16.saturating_add(u16::try_from(*cursor).unwrap_or(u16::MAX)),
                    )),
                )
            }
            AuthModalStep::ApiKeyInput {
                key_buffer,
                cursor,
                masked,
                error,
                ..
            } => {
                let display_key = if *masked {
                    "*".repeat(key_buffer.chars().count())
                } else {
                    key_buffer.clone()
                };
                let mut lines = vec![
                    Line::from("Paste your API key:"),
                    Line::default(),
                    Line::from(format!("  [{display_key}]")),
                    Line::default(),
                ];
                if let Some(message) = error {
                    lines.push(Line::from(Span::styled(
                        message.clone(),
                        Style::default().fg(Color::Red),
                    )));
                    lines.push(Line::default());
                }
                lines.push(Line::from("←/→ move  Enter confirm  Esc back"));
                (
                    Color::Yellow,
                    Text::from(lines),
                    Some((
                        3u16,
                        3u16.saturating_add(u16::try_from(*cursor).unwrap_or(u16::MAX)),
                    )),
                )
            }
            AuthModalStep::OAuthWaiting { status, tick, .. } => {
                const FRAMES: [char; 8] = ['|', '/', '-', '\\', '|', '/', '-', '\\'];
                let spinner = FRAMES[usize::from(*tick) % FRAMES.len()];
                let lines = vec![
                    Line::from(format!("{spinner}  {status}")),
                    Line::default(),
                    Line::from("Esc cancel"),
                ];
                (Color::Blue, Text::from(lines), None)
            }
            AuthModalStep::ModelFetchLoading { provider, .. } => {
                let lines = vec![
                    Line::from(format!("Fetching models for {}...", provider.label())),
                    Line::default(),
                    Line::from("Please wait..."),
                ];
                (Color::Blue, Text::from(lines), None)
            }
            AuthModalStep::ModelSelect { provider, state } => {
                let mut lines = vec![
                    Line::from(format!("Select default model for {}:", provider.label())),
                    Line::default(),
                    Line::from(format!("  Search: {}", state.filter)),
                    Line::default(),
                ];

                let filtered = state.filtered();
                if filtered.is_empty() {
                    lines.push(Line::from(Span::styled(
                        "  (no models found)",
                        Style::default().fg(Color::DarkGray),
                    )));
                } else {
                    let start = state.selected_idx.saturating_sub(5);
                    let end = (start + 10).min(filtered.len());

                    for (i, model) in filtered[start..end].iter().enumerate() {
                        let actual_idx = start + i;
                        let cursor = if actual_idx == state.selected_idx {
                            '>'
                        } else {
                            ' '
                        };
                        let style = if actual_idx == state.selected_idx {
                            Style::default()
                                .fg(Color::Cyan)
                                .add_modifier(ratatui::style::Modifier::BOLD)
                        } else {
                            Style::default()
                        };
                        lines.push(Line::from(Span::styled(
                            format!("  {cursor} {}", model.display_label()),
                            style,
                        )));
                    }
                }

                lines.push(Line::default());
                lines.push(Line::from(
                    "↑/↓ list  ←/→ search  Enter select/input  Esc skip",
                ));
                (
                    Color::Cyan,
                    Text::from(lines),
                    Some((
                        3u16,
                        10u16
                            .saturating_add(u16::try_from(state.filter_cursor).unwrap_or(u16::MAX)),
                    )),
                )
            }
            AuthModalStep::Success { message, .. } => {
                let lines = vec![
                    Line::from(format!("OK {message}")),
                    Line::default(),
                    Line::from("Press any key to continue"),
                ];
                (Color::Green, Text::from(lines), None)
            }
            AuthModalStep::Error { message } => {
                let lines = vec![
                    Line::from(format!("ERR {message}")),
                    Line::default(),
                    Line::from("Press any key to dismiss"),
                ];
                (Color::Red, Text::from(lines), None)
            }
        };

        if area.width <= 92 {
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
                .padding(Padding::new(1, 1, 0, 0));
            let inner = block.inner(area);
            frame.render_widget(Clear, area);
            frame.render_widget(block, area);
            let mut content_lines = text.lines.clone();
            while content_lines
                .last()
                .is_some_and(|line| line.spans.iter().all(|span| span.content.trim().is_empty()))
            {
                content_lines.pop();
            }
            let mut lines = Vec::with_capacity(content_lines.len() + 2);
            lines.push(Line::from(""));
            lines.extend(content_lines);
            lines.push(Line::from(""));
            let paragraph = Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false });
            frame.render_widget(paragraph, inner);
            if let Some((row, col)) = cursor_pos {
                frame.set_cursor_position((
                    inner.x.saturating_add(col),
                    inner.y.saturating_add(row),
                ));
            }
        } else {
            let inner = draw_modal_frame(frame, area, self.title(), border_color);
            let paragraph = Paragraph::new(text).wrap(Wrap { trim: false });
            frame.render_widget(paragraph, inner);
            if let Some((row, col)) = cursor_pos {
                frame.set_cursor_position((
                    inner.x.saturating_add(col),
                    inner.y.saturating_add(row),
                ));
            }
        }
    }

    #[allow(clippy::too_many_lines)]
    fn handle_key(&mut self, key: KeyEvent) -> ModalAction {
        match &mut self.step {
            AuthModalStep::ProviderSelect { selected } => match key.code {
                KeyCode::Up => {
                    *selected = if *selected == 0 { 2 } else { *selected - 1 };
                    ModalAction::Consumed
                }
                KeyCode::Down => {
                    *selected = (*selected + 1) % 3;
                    ModalAction::Consumed
                }
                KeyCode::Enter => {
                    let provider = match *selected {
                        0 => ProviderKind::Anthropic,
                        1 => ProviderKind::OpenAi,
                        _ => ProviderKind::Other,
                    };
                    if provider == ProviderKind::Other {
                        self.step = AuthModalStep::BaseUrlInput {
                            input: String::new(),
                            cursor: 0,
                            error: None,
                        };
                    } else {
                        self.step = AuthModalStep::AuthMethodSelect {
                            provider,
                            selected: 0,
                        };
                    }
                    ModalAction::Consumed
                }
                KeyCode::Esc => ModalAction::Dismiss,
                _ => ModalAction::Consumed,
            },
            AuthModalStep::AuthMethodSelect { provider, selected } => {
                let methods_len = if *provider == ProviderKind::Other {
                    1
                } else {
                    2
                };
                match key.code {
                    KeyCode::Up => {
                        *selected = if *selected == 0 {
                            methods_len - 1
                        } else {
                            *selected - 1
                        };
                        ModalAction::Consumed
                    }
                    KeyCode::Down => {
                        *selected = (*selected + 1) % methods_len;
                        ModalAction::Consumed
                    }
                    KeyCode::Enter => {
                        if *selected == 0 {
                            self.step = AuthModalStep::ApiKeyInput {
                                provider: *provider,
                                base_url: None,
                                key_buffer: String::new(),
                                cursor: 0,
                                masked: true,
                                error: None,
                            };
                        } else {
                            self.step = AuthModalStep::OAuthWaiting {
                                provider: *provider,
                                status: "Preparing OAuth flow...".to_string(),
                                cancel_tx: None,
                                tick: 0,
                            };
                        }
                        ModalAction::Consumed
                    }
                    KeyCode::Esc => {
                        self.step = AuthModalStep::ProviderSelect {
                            selected: match provider {
                                ProviderKind::Anthropic => 0,
                                ProviderKind::OpenAi => 1,
                                ProviderKind::Other => 2,
                            },
                        };
                        ModalAction::Consumed
                    }
                    _ => ModalAction::Consumed,
                }
            }
            AuthModalStep::BaseUrlInput {
                input,
                cursor,
                error,
            } => match key.code {
                KeyCode::Char(ch) => {
                    Self::insert_char_at(input, cursor, ch);
                    *error = None;
                    ModalAction::Consumed
                }
                KeyCode::Backspace => {
                    Self::remove_prev_char(input, cursor);
                    ModalAction::Consumed
                }
                KeyCode::Delete => {
                    Self::remove_current_char(input, *cursor);
                    ModalAction::Consumed
                }
                KeyCode::Left => {
                    *cursor = cursor.saturating_sub(1);
                    ModalAction::Consumed
                }
                KeyCode::Right => {
                    *cursor = (*cursor + 1).min(Self::char_len(input));
                    ModalAction::Consumed
                }
                KeyCode::Home | KeyCode::Up => {
                    *cursor = 0;
                    ModalAction::Consumed
                }
                KeyCode::End | KeyCode::Down => {
                    *cursor = Self::char_len(input);
                    ModalAction::Consumed
                }
                KeyCode::Enter => {
                    if input.is_empty() {
                        *error = Some("Base URL cannot be empty".to_string());
                    } else {
                        self.step = AuthModalStep::ApiKeyInput {
                            provider: ProviderKind::Other,
                            base_url: Some(input.clone()),
                            key_buffer: String::new(),
                            cursor: 0,
                            masked: true,
                            error: None,
                        };
                    }
                    ModalAction::Consumed
                }
                KeyCode::Esc => {
                    self.step = AuthModalStep::ProviderSelect { selected: 2 };
                    ModalAction::Consumed
                }
                _ => ModalAction::Consumed,
            },
            AuthModalStep::ApiKeyInput {
                provider,
                base_url,
                key_buffer,
                cursor,
                error,
                ..
            } => match key.code {
                KeyCode::Char(ch) => {
                    Self::insert_char_at(key_buffer, cursor, ch);
                    *error = None;
                    ModalAction::Consumed
                }
                KeyCode::Backspace => {
                    Self::remove_prev_char(key_buffer, cursor);
                    ModalAction::Consumed
                }
                KeyCode::Delete => {
                    Self::remove_current_char(key_buffer, *cursor);
                    ModalAction::Consumed
                }
                KeyCode::Left => {
                    *cursor = cursor.saturating_sub(1);
                    ModalAction::Consumed
                }
                KeyCode::Right => {
                    *cursor = (*cursor + 1).min(Self::char_len(key_buffer));
                    ModalAction::Consumed
                }
                KeyCode::Home | KeyCode::Up => {
                    *cursor = 0;
                    ModalAction::Consumed
                }
                KeyCode::End | KeyCode::Down => {
                    *cursor = Self::char_len(key_buffer);
                    ModalAction::Consumed
                }
                KeyCode::Enter => {
                    if key_buffer.is_empty() {
                        *error = Some("API key cannot be empty".to_string());
                    } else {
                        Self::save_api_key(*provider, base_url.clone(), key_buffer.clone());
                        self.step = AuthModalStep::ModelFetchLoading {
                            provider: *provider,
                        };
                    }
                    ModalAction::Consumed
                }
                KeyCode::Esc => {
                    if *provider == ProviderKind::Other {
                        let previous = base_url.clone().unwrap_or_default();
                        let previous_len = Self::char_len(&previous);
                        self.step = AuthModalStep::BaseUrlInput {
                            input: previous,
                            cursor: previous_len,
                            error: None,
                        };
                    } else {
                        self.step = AuthModalStep::AuthMethodSelect {
                            provider: *provider,
                            selected: 0,
                        };
                    }
                    ModalAction::Consumed
                }
                _ => ModalAction::Consumed,
            },
            AuthModalStep::OAuthWaiting { cancel_tx, .. } => match key.code {
                KeyCode::Esc => {
                    if let Some(sender) = cancel_tx {
                        let _ = sender.send(());
                    }
                    ModalAction::Dismiss
                }
                _ => ModalAction::Consumed,
            },
            AuthModalStep::ModelFetchLoading { .. } => ModalAction::Consumed,
            AuthModalStep::ModelSelect { provider, state } => match key.code {
                KeyCode::Left => {
                    state.move_cursor_left();
                    ModalAction::Consumed
                }
                KeyCode::Right => {
                    state.move_cursor_right();
                    ModalAction::Consumed
                }
                KeyCode::Up => {
                    state.handle_up();
                    ModalAction::Consumed
                }
                KeyCode::Down => {
                    state.handle_down();
                    ModalAction::Consumed
                }
                KeyCode::Char(c) => {
                    state.handle_char(c);
                    ModalAction::Consumed
                }
                KeyCode::Backspace => {
                    state.handle_backspace();
                    ModalAction::Consumed
                }
                KeyCode::Delete => {
                    state.handle_delete();
                    ModalAction::Consumed
                }
                KeyCode::Home => {
                    state.move_cursor_home();
                    ModalAction::Consumed
                }
                KeyCode::End => {
                    state.move_cursor_end();
                    ModalAction::Consumed
                }
                KeyCode::Enter => {
                    if let Some(model) = state.selected_model() {
                        Self::save_default_model(*provider, &model.id);
                    } else if matches!(*provider, ProviderKind::OpenAi | ProviderKind::Other)
                        && !state.filter.trim().is_empty()
                    {
                        // Allow manual model entry for OpenAI-compatible providers
                        // when remote model listing is not available.
                        Self::save_default_model(*provider, &state.filter);
                    }
                    self.step = AuthModalStep::Success {
                        message: format!("Authenticated as {}", provider.label()),
                    };
                    ModalAction::Consumed
                }
                KeyCode::Esc => {
                    if matches!(*provider, ProviderKind::OpenAi | ProviderKind::Other)
                        && !state.filter.trim().is_empty()
                    {
                        Self::save_default_model(*provider, &state.filter);
                    }
                    self.step = AuthModalStep::Success {
                        message: format!("Authenticated as {}", provider.label()),
                    };
                    ModalAction::Consumed
                }
                _ => ModalAction::Consumed,
            },
            AuthModalStep::Success { .. } | AuthModalStep::Error { .. } => ModalAction::Dismiss,
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

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;

    fn modal() -> AuthModal {
        let (ui_tx, _ui_rx) = mpsc::channel();
        AuthModal::new(ui_tx, None)
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::from(code)
    }

    #[test]
    fn provider_select_initial_state() {
        let modal = modal();
        match &modal.step {
            AuthModalStep::ProviderSelect { selected } => assert_eq!(*selected, 0),
            _ => panic!("expected provider selection step"),
        }
    }

    #[test]
    fn provider_select_to_auth_method() {
        let mut modal = modal();
        assert_eq!(modal.handle_key(key(KeyCode::Enter)), ModalAction::Consumed);
        match &modal.step {
            AuthModalStep::AuthMethodSelect { provider, selected } => {
                assert_eq!(*provider, ProviderKind::Anthropic);
                assert_eq!(*selected, 0);
            }
            _ => panic!("expected auth method select step"),
        }
    }

    #[test]
    fn other_provider_goes_to_base_url_input() {
        let mut modal = modal();
        let _ = modal.handle_key(key(KeyCode::Down));
        let _ = modal.handle_key(key(KeyCode::Down));
        assert_eq!(modal.handle_key(key(KeyCode::Enter)), ModalAction::Consumed);
        match &modal.step {
            AuthModalStep::BaseUrlInput { input, error, .. } => {
                assert!(input.is_empty());
                assert_eq!(error, &None);
            }
            _ => panic!("expected base url input step"),
        }
    }

    #[test]
    fn esc_from_auth_method_goes_back_to_provider_select() {
        let mut modal = modal();
        let _ = modal.handle_key(key(KeyCode::Enter)); // Go to AuthMethodSelect
        assert_eq!(modal.handle_key(key(KeyCode::Esc)), ModalAction::Consumed);
        match &modal.step {
            AuthModalStep::ProviderSelect { selected } => assert_eq!(*selected, 0),
            _ => panic!("expected provider select step"),
        }
    }

    #[test]
    fn esc_from_provider_select_closes_modal() {
        let mut modal = modal();
        assert_eq!(modal.handle_key(key(KeyCode::Esc)), ModalAction::Dismiss);
    }

    #[test]
    fn api_key_input_chars_append() {
        let (ui_tx, _ui_rx) = mpsc::channel();
        let mut modal = AuthModal::new(ui_tx, Some(crate::app::Provider::OpenAi));

        let _ = modal.handle_key(key(KeyCode::Char('s')));
        let _ = modal.handle_key(key(KeyCode::Char('k')));

        match &modal.step {
            AuthModalStep::ApiKeyInput {
                key_buffer, error, ..
            } => {
                assert_eq!(key_buffer, "sk");
                assert_eq!(error, &None);
            }
            _ => panic!("expected api key input step"),
        }
    }

    #[test]
    fn api_key_input_backspace_removes() {
        let (ui_tx, _ui_rx) = mpsc::channel();
        let mut modal = AuthModal::new(ui_tx, Some(crate::app::Provider::OpenAi));
        let _ = modal.handle_key(key(KeyCode::Char('s')));
        let _ = modal.handle_key(key(KeyCode::Char('k')));

        assert_eq!(
            modal.handle_key(key(KeyCode::Backspace)),
            ModalAction::Consumed
        );
        match &modal.step {
            AuthModalStep::ApiKeyInput { key_buffer, .. } => assert_eq!(key_buffer, "s"),
            _ => panic!("expected api key input step"),
        }
    }

    #[test]
    fn api_key_input_empty_enter_rejected() {
        let (ui_tx, _ui_rx) = mpsc::channel();
        let mut modal = AuthModal::new(ui_tx, Some(crate::app::Provider::OpenAi));

        assert_eq!(modal.handle_key(key(KeyCode::Enter)), ModalAction::Consumed);
        match &modal.step {
            AuthModalStep::ApiKeyInput {
                provider,
                key_buffer,
                error,
                ..
            } => {
                assert_eq!(*provider, ProviderKind::OpenAi);
                assert!(key_buffer.is_empty());
                assert_eq!(error.as_deref(), Some("API key cannot be empty"));
            }
            _ => panic!("expected api key input step"),
        }
    }

    #[test]
    fn api_key_input_esc_goes_back() {
        let (ui_tx, _ui_rx) = mpsc::channel();
        let mut modal = AuthModal::new(ui_tx, Some(crate::app::Provider::OpenAi));

        assert_eq!(modal.handle_key(key(KeyCode::Esc)), ModalAction::Consumed);
        match &modal.step {
            AuthModalStep::AuthMethodSelect { provider, .. } => {
                assert_eq!(*provider, ProviderKind::OpenAi);
            }
            _ => panic!("expected auth method select step"),
        }
    }

    #[test]
    fn oauth_waiting_esc_sends_cancel() {
        let (ui_tx, _ui_rx) = mpsc::channel();
        let mut modal = AuthModal::new(ui_tx, Some(crate::app::Provider::Anthropic));
        let (cancel_tx, cancel_rx) = mpsc::channel();
        modal.step = AuthModalStep::OAuthWaiting {
            provider: ProviderKind::Anthropic,
            status: "Preparing OAuth flow...".to_string(),
            cancel_tx: Some(cancel_tx),
            tick: 0,
        };

        assert_eq!(modal.handle_key(key(KeyCode::Esc)), ModalAction::Dismiss);
        assert_eq!(cancel_rx.recv().ok(), Some(()));
    }

    #[test]
    fn success_any_key_dismisses() {
        let mut modal = AuthModal {
            step: AuthModalStep::Success {
                message: "done".to_string(),
            },
        };

        assert_eq!(modal.handle_key(key(KeyCode::Enter)), ModalAction::Dismiss);
    }

    #[test]
    fn error_any_key_dismisses() {
        let mut modal = AuthModal {
            step: AuthModalStep::Error {
                message: "failed".to_string(),
            },
        };

        assert_eq!(
            modal.handle_key(key(KeyCode::Char('x'))),
            ModalAction::Dismiss
        );
    }

    #[test]
    fn new_with_provider_skips_selection() {
        let (ui_tx, _ui_rx) = mpsc::channel();

        let openai_modal = AuthModal::new(ui_tx.clone(), Some(crate::app::Provider::OpenAi));
        assert!(matches!(
            openai_modal.step,
            AuthModalStep::ApiKeyInput {
                provider: ProviderKind::OpenAi,
                ..
            }
        ));

        let anthropic_modal = AuthModal::new(ui_tx, Some(crate::app::Provider::Anthropic));
        assert!(matches!(
            anthropic_modal.step,
            AuthModalStep::OAuthWaiting {
                provider: ProviderKind::Anthropic,
                ..
            }
        ));
    }
}
