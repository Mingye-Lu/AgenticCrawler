use std::sync::mpsc;

use super::*;

fn modal() -> AuthModal {
    let (ui_tx, _ui_rx) = mpsc::channel();
    AuthModal::new(ui_tx, None)
}

fn modal_with_step(step: AuthModalStep) -> AuthModal {
    let (ui_tx, _ui_rx) = mpsc::channel();
    AuthModal {
        step,
        ui_tx,
        model_fetch_in_flight: false,
    }
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
fn wheel_scroll_provider_select_clamps_at_edges() {
    let mut modal = modal();
    let total = flat_preset_list().len();

    modal.handle_vertical_wheel(false);
    match &modal.step {
        AuthModalStep::ProviderSelect { selected } => assert_eq!(*selected, 0),
        _ => panic!("expected provider selection step"),
    }

    for _ in 1..total {
        modal.handle_vertical_wheel(true);
    }
    modal.handle_vertical_wheel(true);
    match &modal.step {
        AuthModalStep::ProviderSelect { selected } => assert_eq!(*selected, total - 1),
        _ => panic!("expected provider selection step"),
    }
}

#[test]
fn provider_select_keyboard_clamps_and_jumps() {
    let mut modal = modal();
    let total = flat_preset_list().len();

    assert_eq!(modal.handle_key(key(KeyCode::Up)), ModalAction::Consumed);
    match &modal.step {
        AuthModalStep::ProviderSelect { selected } => assert_eq!(*selected, 0),
        _ => panic!("expected provider selection step"),
    }

    assert_eq!(modal.handle_key(key(KeyCode::Right)), ModalAction::Consumed);
    match &modal.step {
        AuthModalStep::ProviderSelect { selected } => assert_eq!(*selected, total - 1),
        _ => panic!("expected provider selection step"),
    }

    assert_eq!(modal.handle_key(key(KeyCode::Down)), ModalAction::Consumed);
    match &modal.step {
        AuthModalStep::ProviderSelect { selected } => assert_eq!(*selected, total - 1),
        _ => panic!("expected provider selection step"),
    }

    assert_eq!(modal.handle_key(key(KeyCode::Left)), ModalAction::Consumed);
    match &modal.step {
        AuthModalStep::ProviderSelect { selected } => assert_eq!(*selected, 0),
        _ => panic!("expected provider selection step"),
    }
}

#[test]
fn auth_method_keyboard_clamps_and_jumps() {
    let mut modal = modal();
    let _ = modal.handle_key(key(KeyCode::Enter));

    assert_eq!(modal.handle_key(key(KeyCode::Up)), ModalAction::Consumed);
    match &modal.step {
        AuthModalStep::AuthMethodSelect { selected, .. } => assert_eq!(*selected, 0),
        _ => panic!("expected auth method select step"),
    }

    assert_eq!(modal.handle_key(key(KeyCode::Right)), ModalAction::Consumed);
    match &modal.step {
        AuthModalStep::AuthMethodSelect { selected, .. } => assert_eq!(*selected, 1),
        _ => panic!("expected auth method select step"),
    }

    assert_eq!(modal.handle_key(key(KeyCode::Down)), ModalAction::Consumed);
    match &modal.step {
        AuthModalStep::AuthMethodSelect { selected, .. } => assert_eq!(*selected, 1),
        _ => panic!("expected auth method select step"),
    }

    assert_eq!(modal.handle_key(key(KeyCode::Left)), ModalAction::Consumed);
    match &modal.step {
        AuthModalStep::AuthMethodSelect { selected, .. } => assert_eq!(*selected, 0),
        _ => panic!("expected auth method select step"),
    }
}

#[test]
fn wheel_scroll_model_select_clamps_at_edges() {
    let mut modal = modal_with_step(AuthModalStep::ModelSelect {
        provider: ProviderKind::OpenAi,
        state: crate::tui::model_list::ModelListState {
            models: vec![
                crate::tui::model_list::ModelInfo {
                    id: "a".to_string(),
                    display_name: None,
                },
                crate::tui::model_list::ModelInfo {
                    id: "b".to_string(),
                    display_name: None,
                },
            ],
            filter: String::new(),
            filter_cursor: 0,
            selected_idx: 0,
        },
    });

    modal.handle_vertical_wheel(false);
    if let AuthModalStep::ModelSelect { state, .. } = &modal.step {
        assert_eq!(state.selected_idx, 0);
    } else {
        panic!("expected model select step");
    }

    modal.handle_vertical_wheel(true);
    modal.handle_vertical_wheel(true);
    if let AuthModalStep::ModelSelect { state, .. } = &modal.step {
        assert_eq!(state.selected_idx, 1);
    } else {
        panic!("expected model select step");
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
    let other_idx = flat_preset_list()
        .iter()
        .position(|p| p.id == "other")
        .unwrap();
    for _ in 0..other_idx {
        let _ = modal.handle_key(key(KeyCode::Down));
    }
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
    let _ = modal.handle_key(key(KeyCode::Enter));
    assert_eq!(modal.handle_key(key(KeyCode::Esc)), ModalAction::Consumed);
    match &modal.step {
        AuthModalStep::ProviderSelect { selected } => assert_eq!(*selected, 0),
        _ => panic!("expected provider select step"),
    }
}

#[test]
fn title_matches_through_direct_call_and_dyn_modal() {
    let m = modal();
    // Direct call resolves through the (sole) `Modal::title` impl.
    assert_eq!(m.title(), " Auth ");
    // Calling through `&dyn Modal` must produce the same title -- this is
    // the scenario that broke when a private inherent `title` shadowed
    // the trait method for direct calls but not for dyn dispatch.
    let dyn_modal: &dyn Modal = &m;
    assert_eq!(dyn_modal.title(), Modal::title(&m));

    let openai_modal = modal_with_step(AuthModalStep::AuthMethodSelect {
        provider: ProviderKind::OpenAi,
        selected: 0,
    });
    assert_eq!(openai_modal.title(), " Auth · OpenAI ");
    let dyn_openai: &dyn Modal = &openai_modal;
    assert_eq!(dyn_openai.title(), Modal::title(&openai_modal));
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
            assert_eq!(key_buffer.as_str(), "sk");
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
        AuthModalStep::ApiKeyInput { key_buffer, .. } => {
            assert_eq!(key_buffer.as_str(), "s");
        }
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
fn control_shortcuts_passthrough_modal() {
    let mut modal = modal();
    let ctrl_c = KeyEvent::new(KeyCode::Char('c'), crossterm::event::KeyModifiers::CONTROL);

    assert_eq!(modal.handle_key(ctrl_c), ModalAction::Unhandled);
}

#[test]
fn model_fetch_loading_esc_finishes_auth_without_default_model() {
    let mut modal = modal_with_step(AuthModalStep::ModelFetchLoading {
        provider: ProviderKind::OpenAi,
    });

    assert_eq!(modal.handle_key(key(KeyCode::Esc)), ModalAction::Consumed);
    assert!(matches!(modal.step, AuthModalStep::Success { .. }));
}

#[test]
fn model_select_esc_clears_filter_before_skipping() {
    let mut modal = modal_with_step(AuthModalStep::ModelSelect {
        provider: ProviderKind::OpenAi,
        state: crate::tui::model_list::ModelListState {
            models: vec![],
            filter: "gpt-4.1".to_string(),
            filter_cursor: 7,
            selected_idx: 3,
        },
    });

    assert_eq!(modal.handle_key(key(KeyCode::Esc)), ModalAction::Consumed);
    match &modal.step {
        AuthModalStep::ModelSelect { state, .. } => {
            assert!(state.filter.is_empty());
            assert_eq!(state.filter_cursor, 0);
            assert_eq!(state.selected_idx, 0);
        }
        _ => panic!("expected model select step"),
    }
}

#[test]
fn finish_model_loading_transitions_to_model_select() {
    let mut modal = modal_with_step(AuthModalStep::ModelFetchLoading {
        provider: ProviderKind::Anthropic,
    });

    modal.finish_model_loading(Ok(vec![crate::tui::model_list::ModelInfo {
        id: "claude-sonnet-4-6".to_string(),
        display_name: Some("Claude Sonnet 4.6".to_string()),
    }]));

    match &modal.step {
        AuthModalStep::ModelSelect { provider, state } => {
            assert_eq!(*provider, ProviderKind::Anthropic);
            assert_eq!(state.models.len(), 1);
            assert_eq!(state.models[0].id, "claude-sonnet-4-6");
        }
        _ => panic!("expected model select step"),
    }
}

#[test]
fn success_any_key_dismisses() {
    let mut modal = modal_with_step(AuthModalStep::Success {
        message: "done".to_string(),
    });

    assert_eq!(modal.handle_key(key(KeyCode::Enter)), ModalAction::Dismiss);
}

#[test]
fn error_any_key_dismisses() {
    let mut modal = modal_with_step(AuthModalStep::Error {
        message: "failed".to_string(),
    });

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
