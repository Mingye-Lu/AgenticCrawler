use std::sync::mpsc::Sender;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Paragraph, Wrap};

use crate::app::{provider_label, Provider};
use crate::tui::modal::{draw_modal_frame, Modal, ModalAction};
use crate::tui::ReplTuiEvent;

#[allow(dead_code)]
pub(crate) enum AuthModalStep {
    ProviderSelect {
        selected: usize,
        providers: Vec<(Provider, &'static str)>,
    },
    ApiKeyInput {
        provider: Provider,
        key_buffer: String,
        error: Option<String>,
    },
    OAuthWaiting {
        provider: Provider,
        status: String,
        cancel_tx: Option<Sender<()>>,
        tick: u8,
    },
    Success {
        provider: Provider,
        message: String,
    },
    Error {
        message: String,
    },
}

pub(crate) struct AuthModal {
    pub(crate) step: AuthModalStep,
    #[allow(dead_code)]
    ui_tx: Sender<ReplTuiEvent>,
}

impl AuthModal {
    /// Create auth modal. If provider is given, skip `ProviderSelect`.
    pub(crate) fn new(ui_tx: Sender<ReplTuiEvent>, provider: Option<Provider>) -> Self {
        let step = if let Some(p) = provider {
            match p {
                Provider::OpenAi => AuthModalStep::ApiKeyInput {
                    provider: p,
                    key_buffer: String::new(),
                    error: None,
                },
                _ => AuthModalStep::OAuthWaiting {
                    provider: p,
                    status: "Preparing OAuth flow...".to_string(),
                    cancel_tx: None,
                    tick: 0,
                },
            }
        } else {
            AuthModalStep::ProviderSelect {
                selected: 0,
                providers: vec![
                    (Provider::Anthropic, "Anthropic (OAuth)"),
                    (Provider::OpenAi, "OpenAI (API key)"),
                    (Provider::Codex, "Codex (OAuth)"),
                ],
            }
        };

        Self { step, ui_tx }
    }
}

impl Modal for AuthModal {
    fn draw(&self, frame: &mut ratatui::Frame<'_>, area: Rect) {
        let (border_color, text) = match &self.step {
            AuthModalStep::ProviderSelect {
                selected,
                providers,
            } => {
                let mut lines = providers
                    .iter()
                    .enumerate()
                    .map(|(index, (_, display_name))| {
                        let cursor = if index == *selected { '▸' } else { ' ' };
                        Line::from(format!("  {cursor} {display_name}"))
                    })
                    .collect::<Vec<_>>();
                lines.push(Line::default());
                lines.push(Line::from("↑/↓ navigate  Enter select  Esc cancel"));
                (Color::Cyan, Text::from(lines))
            }
            AuthModalStep::ApiKeyInput {
                key_buffer, error, ..
            } => {
                let masked = "•".repeat(key_buffer.chars().count());
                let mut lines = vec![
                    Line::from("Paste your API key:"),
                    Line::default(),
                    Line::from(format!("  [{masked}]")),
                    Line::default(),
                ];
                if let Some(message) = error {
                    lines.push(Line::from(Span::styled(
                        message.clone(),
                        Style::default().fg(Color::Red),
                    )));
                    lines.push(Line::default());
                }
                lines.push(Line::from("Enter confirm  Esc cancel"));
                (Color::Yellow, Text::from(lines))
            }
            AuthModalStep::OAuthWaiting { status, tick, .. } => {
                const FRAMES: [char; 8] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧'];
                let spinner = FRAMES[usize::from(*tick) % FRAMES.len()];
                let lines = vec![
                    Line::from(format!("{spinner}  {status}")),
                    Line::default(),
                    Line::from("Esc cancel"),
                ];
                (Color::Blue, Text::from(lines))
            }
            AuthModalStep::Success { message, .. } => {
                let lines = vec![
                    Line::from(format!("✓ {message}")),
                    Line::default(),
                    Line::from("Press any key to continue"),
                ];
                (Color::Green, Text::from(lines))
            }
            AuthModalStep::Error { message } => {
                let lines = vec![
                    Line::from(format!("✗ {message}")),
                    Line::default(),
                    Line::from("Press any key to dismiss"),
                ];
                (Color::Red, Text::from(lines))
            }
        };

        let inner = draw_modal_frame(frame, area, self.title(), border_color);
        let paragraph = Paragraph::new(text).wrap(Wrap { trim: false });
        frame.render_widget(paragraph, inner);
    }

    fn handle_key(&mut self, key: KeyEvent) -> ModalAction {
        match &mut self.step {
            AuthModalStep::ProviderSelect {
                selected,
                providers,
            } => match key.code {
                KeyCode::Up => {
                    *selected = if *selected == 0 {
                        providers.len().saturating_sub(1)
                    } else {
                        *selected - 1
                    };
                    ModalAction::Consumed
                }
                KeyCode::Down => {
                    *selected = (*selected + 1) % providers.len();
                    ModalAction::Consumed
                }
                KeyCode::Enter => {
                    let provider = providers[*selected].0;
                    self.step = match provider {
                        Provider::OpenAi => AuthModalStep::ApiKeyInput {
                            provider,
                            key_buffer: String::new(),
                            error: None,
                        },
                        Provider::Anthropic | Provider::Codex => AuthModalStep::OAuthWaiting {
                            provider,
                            status: "Preparing OAuth flow...".to_string(),
                            cancel_tx: None,
                            tick: 0,
                        },
                    };
                    ModalAction::Consumed
                }
                KeyCode::Esc => ModalAction::Dismiss,
                _ => ModalAction::Consumed,
            },
            AuthModalStep::ApiKeyInput {
                provider,
                key_buffer,
                error,
            } => match key.code {
                KeyCode::Char(ch) => {
                    key_buffer.push(ch);
                    *error = None;
                    ModalAction::Consumed
                }
                KeyCode::Backspace => {
                    key_buffer.pop();
                    ModalAction::Consumed
                }
                KeyCode::Enter => {
                    if key_buffer.is_empty() {
                        *error = Some("API key cannot be empty".to_string());
                    } else {
                        self.step = AuthModalStep::Success {
                            provider: *provider,
                            message: format!("API key accepted for {}", provider_label(*provider)),
                        };
                    }
                    ModalAction::Consumed
                }
                KeyCode::Esc => ModalAction::Dismiss,
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
            AuthModalStep::Success { .. } | AuthModalStep::Error { .. } => ModalAction::Dismiss,
        }
    }

    fn title(&self) -> &str {
        match &self.step {
            AuthModalStep::ProviderSelect { .. } => " Auth ",
            AuthModalStep::ApiKeyInput { provider, .. } => match provider {
                Provider::Anthropic => " Auth · Anthropic ",
                Provider::OpenAi => " Auth · OpenAI ",
                Provider::Codex => " Auth · Codex ",
            },
            AuthModalStep::OAuthWaiting { .. } => " Auth · Waiting ",
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
    fn provider_select_arrow_keys_move_cursor() {
        let mut modal = modal();

        assert_eq!(modal.handle_key(key(KeyCode::Down)), ModalAction::Consumed);
        match &modal.step {
            AuthModalStep::ProviderSelect { selected, .. } => assert_eq!(*selected, 1),
            _ => panic!("expected provider selection step"),
        }

        assert_eq!(modal.handle_key(key(KeyCode::Up)), ModalAction::Consumed);
        match &modal.step {
            AuthModalStep::ProviderSelect { selected, .. } => assert_eq!(*selected, 0),
            _ => panic!("expected provider selection step"),
        }
    }

    #[test]
    fn provider_select_enter_transitions_to_api_key_for_openai() {
        let mut modal = modal();
        let _ = modal.handle_key(key(KeyCode::Down));

        assert_eq!(modal.handle_key(key(KeyCode::Enter)), ModalAction::Consumed);
        match &modal.step {
            AuthModalStep::ApiKeyInput {
                provider,
                key_buffer,
                error,
            } => {
                assert_eq!(*provider, Provider::OpenAi);
                assert!(key_buffer.is_empty());
                assert_eq!(error, &None);
            }
            _ => panic!("expected api key input step"),
        }
    }

    #[test]
    fn provider_select_enter_transitions_to_oauth_for_anthropic() {
        let mut modal = modal();

        assert_eq!(modal.handle_key(key(KeyCode::Enter)), ModalAction::Consumed);
        match &modal.step {
            AuthModalStep::OAuthWaiting {
                provider,
                status,
                cancel_tx,
                tick,
            } => {
                assert_eq!(*provider, Provider::Anthropic);
                assert_eq!(status, "Preparing OAuth flow...");
                assert!(cancel_tx.is_none());
                assert_eq!(*tick, 0);
            }
            _ => panic!("expected oauth waiting step"),
        }
    }

    #[test]
    fn api_key_input_chars_append() {
        let (ui_tx, _ui_rx) = mpsc::channel();
        let mut modal = AuthModal::new(ui_tx, Some(Provider::OpenAi));

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
        let mut modal = AuthModal::new(ui_tx, Some(Provider::OpenAi));
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
        let mut modal = AuthModal::new(ui_tx, Some(Provider::OpenAi));

        assert_eq!(modal.handle_key(key(KeyCode::Enter)), ModalAction::Consumed);
        match &modal.step {
            AuthModalStep::ApiKeyInput {
                provider,
                key_buffer,
                error,
            } => {
                assert_eq!(*provider, Provider::OpenAi);
                assert!(key_buffer.is_empty());
                assert_eq!(error.as_deref(), Some("API key cannot be empty"));
            }
            _ => panic!("expected api key input step"),
        }
    }

    #[test]
    fn api_key_input_esc_dismisses() {
        let (ui_tx, _ui_rx) = mpsc::channel();
        let mut modal = AuthModal::new(ui_tx, Some(Provider::OpenAi));

        assert_eq!(modal.handle_key(key(KeyCode::Esc)), ModalAction::Dismiss);
    }

    #[test]
    fn oauth_waiting_esc_sends_cancel() {
        let (ui_tx, _ui_rx) = mpsc::channel();
        let mut modal = AuthModal::new(ui_tx, Some(Provider::Anthropic));
        let (cancel_tx, cancel_rx) = mpsc::channel();
        modal.step = AuthModalStep::OAuthWaiting {
            provider: Provider::Anthropic,
            status: "Preparing OAuth flow...".to_string(),
            cancel_tx: Some(cancel_tx),
            tick: 0,
        };

        assert_eq!(modal.handle_key(key(KeyCode::Esc)), ModalAction::Dismiss);
        assert_eq!(cancel_rx.recv().ok(), Some(()));
    }

    #[test]
    fn success_any_key_dismisses() {
        let (ui_tx, _ui_rx) = mpsc::channel();
        let mut modal = AuthModal {
            step: AuthModalStep::Success {
                provider: Provider::OpenAi,
                message: "done".to_string(),
            },
            ui_tx,
        };

        assert_eq!(modal.handle_key(key(KeyCode::Enter)), ModalAction::Dismiss);
    }

    #[test]
    fn error_any_key_dismisses() {
        let (ui_tx, _ui_rx) = mpsc::channel();
        let mut modal = AuthModal {
            step: AuthModalStep::Error {
                message: "failed".to_string(),
            },
            ui_tx,
        };

        assert_eq!(
            modal.handle_key(key(KeyCode::Char('x'))),
            ModalAction::Dismiss
        );
    }

    #[test]
    fn api_key_masking_renders_dots() {
        let key_buffer = "sk-ant-abc123XYZ";
        let masked = "•".repeat(key_buffer.chars().count());
        assert_eq!(masked.chars().count(), key_buffer.chars().count());
        assert!(masked.chars().all(|c| c == '•'));
        assert!(!masked.contains('s'));
        assert!(!masked.contains('k'));
    }

    #[test]
    fn browser_fail_shows_url() {
        let url = "https://console.anthropic.com/oauth/authorize?code_challenge=abc";
        let error_str = "permission denied";
        let message = format!("Browser failed. Visit: {url}  ({error_str})");
        assert!(message.contains(url));
        assert!(message.contains(error_str));
        assert!(message.starts_with("Browser failed"));
    }

    #[test]
    fn oauth_error_state_displays_message() {
        let (ui_tx, _ui_rx) = mpsc::channel();
        let mut modal = AuthModal {
            step: AuthModalStep::Error {
                message: "OAuth callback timed out after 5 minutes".to_string(),
            },
            ui_tx,
        };
        assert_eq!(modal.title(), " Auth · Error ");
        assert_eq!(modal.handle_key(key(KeyCode::Enter)), ModalAction::Dismiss);
    }

    #[test]
    fn oauth_waiting_status_can_be_updated() {
        let (ui_tx, _ui_rx) = mpsc::channel();
        let mut modal = AuthModal::new(ui_tx, Some(Provider::Anthropic));
        if let AuthModalStep::OAuthWaiting { ref mut status, .. } = modal.step {
            *status = "Waiting for OAuth callback on port 4545…".to_string();
        }
        match &modal.step {
            AuthModalStep::OAuthWaiting { status, .. } => {
                assert!(status.contains("4545"));
            }
            _ => panic!("expected OAuthWaiting"),
        }
    }

    #[test]
    fn new_with_provider_skips_selection() {
        let (ui_tx, _ui_rx) = mpsc::channel();

        let openai_modal = AuthModal::new(ui_tx.clone(), Some(Provider::OpenAi));
        assert!(matches!(
            openai_modal.step,
            AuthModalStep::ApiKeyInput {
                provider: Provider::OpenAi,
                ..
            }
        ));

        let anthropic_modal = AuthModal::new(ui_tx, Some(Provider::Anthropic));
        assert!(matches!(
            anthropic_modal.step,
            AuthModalStep::OAuthWaiting {
                provider: Provider::Anthropic,
                ..
            }
        ));
    }
}
