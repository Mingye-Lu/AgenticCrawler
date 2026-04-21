use super::auth_modal::AuthModal;
use super::modal::{Modal, ModalAction};
use crossterm::event::KeyEvent;
use ratatui::layout::Rect;
use ratatui::Frame;

pub enum ActiveModal {
    Auth(AuthModal),
    // Model(ModelModal)  -- will be added in Task 3
}

impl Modal for ActiveModal {
    fn draw(&self, frame: &mut Frame<'_>, area: Rect) {
        match self {
            Self::Auth(modal) => modal.draw(frame, area),
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> ModalAction {
        match self {
            Self::Auth(modal) => modal.handle_key(key),
        }
    }

    fn title(&self) -> &str {
        match self {
            Self::Auth(modal) => modal.title(),
        }
    }
}

impl ActiveModal {
    pub fn supports_vertical_wheel(&self) -> bool {
        match self {
            Self::Auth(modal) => modal.supports_vertical_wheel(),
        }
    }

    pub fn handle_vertical_wheel(&mut self, down: bool) {
        match self {
            Self::Auth(modal) => modal.handle_vertical_wheel(down),
        }
    }

    pub fn process_loading(&mut self) {
        match self {
            Self::Auth(modal) => modal.process_loading(),
        }
    }

    pub fn as_auth(&self) -> &AuthModal {
        match self {
            Self::Auth(modal) => modal,
        }
    }

    pub fn as_auth_mut(&mut self) -> &mut AuthModal {
        match self {
            Self::Auth(modal) => modal,
        }
    }
}
