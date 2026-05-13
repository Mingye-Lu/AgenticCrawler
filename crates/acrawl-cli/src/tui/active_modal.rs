use super::auth_modal::AuthModal;
use super::modal::{Modal, ModalAction};
use super::model_modal::ModelModal;
use super::session_modal::SessionModal;
use crossterm::event::KeyEvent;
use ratatui::layout::Rect;
use ratatui::Frame;

pub enum ActiveModal {
    Auth(AuthModal),
    Model(ModelModal),
    Session(SessionModal),
}

impl Modal for ActiveModal {
    fn draw(&self, frame: &mut Frame<'_>, area: Rect) {
        match self {
            Self::Auth(modal) => modal.draw(frame, area),
            Self::Model(modal) => modal.draw(frame, area),
            Self::Session(modal) => modal.draw(frame, area),
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> ModalAction {
        match self {
            Self::Auth(modal) => modal.handle_key(key),
            Self::Model(modal) => modal.handle_key(key),
            Self::Session(modal) => modal.handle_key(key),
        }
    }

    fn title(&self) -> &str {
        match self {
            Self::Auth(modal) => modal.title(),
            Self::Model(modal) => modal.title(),
            Self::Session(modal) => modal.title(),
        }
    }
}

impl ActiveModal {
    pub fn supports_vertical_wheel(&self) -> bool {
        match self {
            Self::Auth(modal) => modal.supports_vertical_wheel(),
            Self::Model(modal) => modal.supports_vertical_wheel(),
            Self::Session(modal) => modal.supports_vertical_wheel(),
        }
    }

    pub fn handle_vertical_wheel(&mut self, down: bool) {
        match self {
            Self::Auth(modal) => modal.handle_vertical_wheel(down),
            Self::Model(modal) => modal.handle_vertical_wheel(down),
            Self::Session(modal) => modal.handle_vertical_wheel(down),
        }
    }

    pub fn process_loading(&mut self) {
        match self {
            Self::Auth(modal) => modal.process_loading(),
            Self::Model(_) | Self::Session(_) => {}
        }
    }

    pub fn as_auth(&self) -> Option<&AuthModal> {
        match self {
            Self::Auth(modal) => Some(modal),
            _ => None,
        }
    }

    pub fn as_auth_mut(&mut self) -> Option<&mut AuthModal> {
        match self {
            Self::Auth(modal) => Some(modal),
            _ => None,
        }
    }

    pub fn as_model(&self) -> Option<&ModelModal> {
        match self {
            Self::Model(modal) => Some(modal),
            _ => None,
        }
    }

    #[allow(dead_code)]
    pub fn as_session(&self) -> Option<&SessionModal> {
        match self {
            Self::Session(modal) => Some(modal),
            _ => None,
        }
    }

    pub fn as_session_mut(&mut self) -> Option<&mut SessionModal> {
        match self {
            Self::Session(modal) => Some(modal),
            _ => None,
        }
    }
}
