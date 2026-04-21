use super::auth_modal::AuthModal;
use super::modal::{Modal, ModalAction};
use super::model_modal::ModelModal;
use crossterm::event::KeyEvent;
use ratatui::layout::Rect;
use ratatui::Frame;

pub enum ActiveModal {
    Auth(AuthModal),
    #[allow(dead_code)]
    Model(ModelModal),
}

impl Modal for ActiveModal {
    fn draw(&self, frame: &mut Frame<'_>, area: Rect) {
        match self {
            Self::Auth(modal) => modal.draw(frame, area),
            Self::Model(modal) => modal.draw(frame, area),
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> ModalAction {
        match self {
            Self::Auth(modal) => modal.handle_key(key),
            Self::Model(modal) => modal.handle_key(key),
        }
    }

    fn title(&self) -> &str {
        match self {
            Self::Auth(modal) => modal.title(),
            Self::Model(modal) => modal.title(),
        }
    }
}

impl ActiveModal {
    pub fn supports_vertical_wheel(&self) -> bool {
        match self {
            Self::Auth(modal) => modal.supports_vertical_wheel(),
            Self::Model(modal) => modal.supports_vertical_wheel(),
        }
    }

    pub fn handle_vertical_wheel(&mut self, down: bool) {
        match self {
            Self::Auth(modal) => modal.handle_vertical_wheel(down),
            Self::Model(modal) => modal.handle_vertical_wheel(down),
        }
    }

    pub fn process_loading(&mut self) {
        match self {
            Self::Auth(modal) => modal.process_loading(),
            Self::Model(_) => {}
        }
    }

    pub fn as_auth(&self) -> Option<&AuthModal> {
        match self {
            Self::Auth(modal) => Some(modal),
            Self::Model(_) => None,
        }
    }

    pub fn as_auth_mut(&mut self) -> Option<&mut AuthModal> {
        match self {
            Self::Auth(modal) => Some(modal),
            Self::Model(_) => None,
        }
    }

    pub fn as_model(&self) -> Option<&ModelModal> {
        match self {
            Self::Auth(_) => None,
            Self::Model(modal) => Some(modal),
        }
    }

    #[allow(dead_code)]
    pub fn as_model_mut(&mut self) -> Option<&mut ModelModal> {
        match self {
            Self::Auth(_) => None,
            Self::Model(modal) => Some(modal),
        }
    }
}
