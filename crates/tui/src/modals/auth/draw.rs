use super::*;

pub(super) struct DrawState {
    pub(super) border_color: Color,
    pub(super) body_lines: Vec<Line<'static>>,
    pub(super) footer_hint: Option<Line<'static>>,
    pub(super) cursor_pos: Option<(u16, u16)>,
    pub(super) anchor_line: Option<usize>,
}

impl AuthModal {
    #[allow(clippy::too_many_lines)]
    pub(super) fn build_draw_state(&self) -> DrawState {
        let hint_style = Style::default()
            .fg(Color::Rgb(130, 136, 145))
            .add_modifier(Modifier::DIM);
        let hint_line = |text: &str| Line::from(Span::styled(text.to_string(), hint_style));

        let (border_color, body_lines, footer_hint, cursor_pos, anchor_line) = match &self.step {
            AuthModalStep::ProviderSelect { selected } => {
                let presets = flat_preset_list();
                let mut lines: Vec<Line<'static>> = Vec::new();
                let mut idx = 0usize;
                let mut selected_line: usize = 0;
                let categories: &[(api::ProviderCategory, &str)] = &[
                    (
                        api::ProviderCategory::Popular,
                        "─── Popular ───────────────────────────────",
                    ),
                    (
                        api::ProviderCategory::OssHosting,
                        "─── Open Source Hosting ───────────────────",
                    ),
                    (
                        api::ProviderCategory::Specialized,
                        "─── Specialized ────────────────────────────",
                    ),
                    (
                        api::ProviderCategory::Enterprise,
                        "─── Enterprise ─────────────────────────────",
                    ),
                    (
                        api::ProviderCategory::Gateway,
                        "─── Routing / Gateway ──────────────────────",
                    ),
                    (
                        api::ProviderCategory::Other,
                        "─── Other ──────────────────────────────────",
                    ),
                ];
                for (cat, header) in categories {
                    let group: Vec<_> = presets.iter().filter(|p| p.category == *cat).collect();
                    if group.is_empty() {
                        continue;
                    }
                    lines.push(Line::from(Span::styled(
                        *header,
                        Style::default().fg(Color::DarkGray),
                    )));
                    for p in &group {
                        if idx == *selected {
                            selected_line = lines.len();
                        }
                        let cursor = if idx == *selected { '\u{25B8}' } else { ' ' };
                        let style = if idx == *selected {
                            Style::default()
                                .fg(Color::Cyan)
                                .add_modifier(ratatui::style::Modifier::BOLD)
                        } else {
                            Style::default()
                        };
                        lines.push(Line::from(Span::styled(
                            format!("  {cursor} {}", p.display_name),
                            style,
                        )));
                        idx += 1;
                    }
                }
                (
                    Color::Cyan,
                    lines,
                    Some(hint_line(
                        "ↀↀnavigate  ↀfirst  ↀlast  Enter select  Esc cancel",
                    )),
                    None,
                    Some(selected_line),
                )
            }
            AuthModalStep::AuthMethodSelect { provider, selected } => {
                let methods = match provider {
                    ProviderKind::Anthropic => vec!["API Key", "OAuth"],
                    ProviderKind::OpenAi => vec!["API Key", "OAuth (Codex)"],
                    ProviderKind::Other | ProviderKind::Preset(_) => vec!["API Key"],
                };
                let mut lines: Vec<Line<'static>> = vec![
                    Line::from(format!("Select auth method for {}:", provider.label())),
                    Line::default(),
                ];
                for (index, method) in methods.iter().enumerate() {
                    let cursor = if index == *selected { '>' } else { ' ' };
                    lines.push(Line::from(format!("  {cursor} {method}")));
                }
                (
                    Color::Cyan,
                    lines,
                    Some(hint_line(
                        "Up/Down navigate  Left first  Right last  Enter select  Esc back",
                    )),
                    None,
                    Some(selected.saturating_add(2)),
                )
            }
            AuthModalStep::BaseUrlInput {
                provider,
                input,
                cursor,
                error,
            } => {
                let header = if matches!(provider, ProviderKind::Other) {
                    "Enter base URL for Other provider:"
                } else {
                    "Base URL (replace placeholders):"
                };
                let mut lines: Vec<Line<'static>> = vec![
                    Line::from(header),
                    Line::default(),
                    Line::from(format!("  > {input}")),
                    Line::default(),
                ];
                if let Some(message) = error {
                    lines.push(Line::from(Span::styled(
                        message.clone(),
                        Style::default().fg(Color::Red),
                    )));
                }
                (
                    Color::Yellow,
                    lines,
                    Some(hint_line("ↀↀmove  Enter confirm  Esc back")),
                    Some((
                        3u16,
                        u16::try_from(
                            text_display_width("  > ") + prefix_display_width(input, *cursor),
                        )
                        .unwrap_or(u16::MAX),
                    )),
                    None,
                )
            }
            AuthModalStep::ApiKeyInput {
                provider,
                base_url,
                key_buffer,
                cursor,
                masked,
                error,
            } => {
                let display_key: String = if *masked {
                    "*".repeat(key_buffer.chars().count())
                } else {
                    (**key_buffer).clone()
                };
                let preset_url = match provider {
                    ProviderKind::Preset(p) => Some(p.base_url),
                    _ => None,
                };
                let effective_url = base_url.as_deref().or(preset_url);
                let mut lines: Vec<Line<'static>> = vec![
                    Line::from("Paste your API key:"),
                    Line::default(),
                    Line::from(format!("  [{display_key}]")),
                    Line::default(),
                ];
                if let Some(url) = effective_url {
                    lines.push(Line::from(Span::styled(
                        format!("  URL: {url}"),
                        Style::default()
                            .fg(Color::Rgb(130, 136, 145))
                            .add_modifier(Modifier::DIM),
                    )));
                }
                let key_len = key_buffer.chars().count();
                if key_len > 0 {
                    lines.push(Line::from(Span::styled(
                        format!("  {key_len} characters"),
                        Style::default()
                            .fg(Color::Rgb(130, 136, 145))
                            .add_modifier(Modifier::DIM),
                    )));
                }
                if let Some(message) = error {
                    lines.push(Line::from(Span::styled(
                        message.clone(),
                        Style::default().fg(Color::Red),
                    )));
                }
                (
                    Color::Yellow,
                    lines,
                    Some(hint_line("ↀↀmove  Ctrl+V paste  Enter confirm  Esc back")),
                    Some((
                        3u16,
                        u16::try_from(
                            text_display_width("  [") + prefix_display_width(&display_key, *cursor),
                        )
                        .unwrap_or(u16::MAX),
                    )),
                    None,
                )
            }
            AuthModalStep::OAuthWaiting { status, tick, .. } => {
                const FRAMES: [char; 8] = ['|', '/', '-', '\\', '|', '/', '-', '\\'];
                let spinner = FRAMES[usize::from(*tick) % FRAMES.len()];
                let lines = vec![Line::from(format!("{spinner}  {status}"))];
                (
                    Color::Blue,
                    lines,
                    Some(hint_line("Esc cancel")),
                    None,
                    None,
                )
            }
            AuthModalStep::ModelFetchLoading { provider, .. } => {
                let lines = vec![
                    Line::from(format!("Fetching models for {}...", provider.label())),
                    Line::default(),
                    Line::from("Please wait..."),
                ];
                (
                    Color::Blue,
                    lines,
                    Some(hint_line("Esc finish without choosing a default model")),
                    None,
                    None,
                )
            }
            AuthModalStep::ModelSelect { provider, state } => {
                let mut lines: Vec<Line<'static>> = vec![
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
                    let visible_rows = 10usize;
                    let start = state
                        .selected_idx
                        .saturating_sub(visible_rows.saturating_sub(1));
                    let end = (start + visible_rows).min(filtered.len());

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
                            format!("  {cursor} {}", crate::model_list::display_label(model)),
                            style,
                        )));
                    }
                }
                (
                    Color::Cyan,
                    lines,
                    Some(hint_line(
                        "ↀↀlist  ↀↀsearch  Enter select/input  Esc clear/skip",
                    )),
                    Some((
                        3u16,
                        u16::try_from(
                            text_display_width("  Search: ")
                                + prefix_display_width(&state.filter, state.filter_cursor),
                        )
                        .unwrap_or(u16::MAX),
                    )),
                    None,
                )
            }
            AuthModalStep::Success { message, .. } => {
                let lines = vec![
                    Line::from(format!("OK {message}")),
                    Line::default(),
                    Line::from("Press any key to continue"),
                ];
                (Color::Green, lines, None, None, None)
            }
            AuthModalStep::Error { message } => {
                let lines = vec![
                    Line::from(format!("ERR {message}")),
                    Line::default(),
                    Line::from("Press any key to dismiss"),
                ];
                (Color::Red, lines, None, None, None)
            }
        };

        DrawState {
            border_color,
            body_lines,
            footer_hint,
            cursor_pos,
            anchor_line,
        }
    }
}
