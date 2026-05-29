use super::*;

impl AuthModal {
    #[allow(clippy::too_many_lines)]
    pub(super) fn handle_key_by_step(&mut self, key: KeyEvent) -> ModalAction {
        match &mut self.step {
            AuthModalStep::ProviderSelect { selected } => {
                let total = flat_preset_list().len();
                match key.code {
                    KeyCode::Up => {
                        *selected = selected.saturating_sub(1);
                        ModalAction::Consumed
                    }
                    KeyCode::Down => {
                        *selected = (*selected + 1).min(total.saturating_sub(1));
                        ModalAction::Consumed
                    }
                    KeyCode::Left => {
                        *selected = 0;
                        ModalAction::Consumed
                    }
                    KeyCode::Right => {
                        *selected = total.saturating_sub(1);
                        ModalAction::Consumed
                    }
                    KeyCode::Enter => {
                        let preset = flat_preset_list()[*selected];
                        match preset.id {
                            "anthropic" => {
                                self.step = AuthModalStep::AuthMethodSelect {
                                    provider: ProviderKind::Anthropic,
                                    selected: 0,
                                };
                            }
                            "openai" => {
                                self.step = AuthModalStep::AuthMethodSelect {
                                    provider: ProviderKind::OpenAi,
                                    selected: 0,
                                };
                            }
                            "other" => {
                                self.step = AuthModalStep::BaseUrlInput {
                                    provider: ProviderKind::Other,
                                    input: String::new(),
                                    cursor: 0,
                                    error: None,
                                };
                            }
                            _ => {
                                self.step = AuthModalStep::ApiKeyInput {
                                    provider: ProviderKind::Preset(preset),
                                    base_url: None,
                                    key_buffer: Zeroizing::new(String::new()),
                                    cursor: 0,
                                    masked: true,
                                    error: None,
                                };
                            }
                        }
                        ModalAction::Consumed
                    }
                    KeyCode::Esc => ModalAction::Dismiss,
                    _ => ModalAction::Consumed,
                }
            }
            AuthModalStep::AuthMethodSelect { provider, selected } => {
                let methods_len = Self::auth_method_count(*provider);
                match key.code {
                    KeyCode::Up => {
                        *selected = selected.saturating_sub(1);
                        ModalAction::Consumed
                    }
                    KeyCode::Down => {
                        *selected = (*selected + 1).min(methods_len - 1);
                        ModalAction::Consumed
                    }
                    KeyCode::Left => {
                        *selected = 0;
                        ModalAction::Consumed
                    }
                    KeyCode::Right => {
                        *selected = methods_len - 1;
                        ModalAction::Consumed
                    }
                    KeyCode::Enter => {
                        if *selected == 0 {
                            self.step = AuthModalStep::ApiKeyInput {
                                provider: *provider,
                                base_url: None,
                                key_buffer: Zeroizing::new(String::new()),
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
                            selected: Self::provider_select_index(*provider),
                        };
                        ModalAction::Consumed
                    }
                    _ => ModalAction::Consumed,
                }
            }
            AuthModalStep::BaseUrlInput {
                provider,
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
                            provider: *provider,
                            base_url: Some(input.clone()),
                            key_buffer: Zeroizing::new(String::new()),
                            cursor: 0,
                            masked: true,
                            error: None,
                        };
                    }
                    ModalAction::Consumed
                }
                KeyCode::Esc => {
                    self.step = AuthModalStep::ProviderSelect {
                        selected: Self::provider_select_index(*provider),
                    };
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
                        match provider {
                            ProviderKind::Other => {
                                self.step = AuthModalStep::Success {
                                    message: format!("Authenticated as {}", provider.label()),
                                };
                            }
                            _ => {
                                self.step = AuthModalStep::ModelFetchLoading {
                                    provider: *provider,
                                };
                            }
                        }
                    }
                    ModalAction::Consumed
                }
                KeyCode::Esc => {
                    if *provider == ProviderKind::Other {
                        let previous = base_url.clone().unwrap_or_default();
                        let previous_len = Self::char_len(&previous);
                        self.step = AuthModalStep::BaseUrlInput {
                            provider: ProviderKind::Other,
                            input: previous,
                            cursor: previous_len,
                            error: None,
                        };
                    } else if let ProviderKind::Preset(p) = provider {
                        if base_url.is_some() {
                            let previous = base_url.clone().unwrap_or_default();
                            let previous_len = Self::char_len(&previous);
                            self.step = AuthModalStep::BaseUrlInput {
                                provider: ProviderKind::Preset(*p),
                                input: previous,
                                cursor: previous_len,
                                error: None,
                            };
                        } else {
                            self.step = AuthModalStep::ProviderSelect {
                                selected: Self::provider_select_index(*provider),
                            };
                        }
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
            AuthModalStep::ModelFetchLoading { provider } => match key.code {
                KeyCode::Esc => {
                    self.step = AuthModalStep::Success {
                        message: format!("Authenticated as {}", provider.label()),
                    };
                    ModalAction::Consumed
                }
                _ => ModalAction::Consumed,
            },
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
                    } else if matches!(
                        *provider,
                        ProviderKind::OpenAi | ProviderKind::Other | ProviderKind::Preset(_)
                    ) && !state.filter.trim().is_empty()
                    {
                        Self::save_default_model(*provider, &state.filter);
                    }
                    self.step = AuthModalStep::Success {
                        message: format!("Authenticated as {}", provider.label()),
                    };
                    ModalAction::Consumed
                }
                KeyCode::Esc => {
                    if state.filter.trim().is_empty() {
                        self.step = AuthModalStep::Success {
                            message: format!("Authenticated as {}", provider.label()),
                        };
                    } else {
                        state.filter.clear();
                        state.filter_cursor = 0;
                        state.selected_idx = 0;
                    }
                    ModalAction::Consumed
                }
                _ => ModalAction::Consumed,
            },
            AuthModalStep::Success { .. } | AuthModalStep::Error { .. } => ModalAction::Dismiss,
        }
    }
}
