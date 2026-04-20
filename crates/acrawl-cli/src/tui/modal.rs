//! Base modal trait and shared rendering helpers for the Ratatui TUI modal system.

use crossterm::event::KeyEvent;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Padding};

/// Represents the action taken by a modal in response to a key event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModalAction {
    /// Key was handled by the modal; keep it open.
    Consumed,
    /// Modal should close.
    Dismiss,
}

/// Base trait for modal dialogs in the TUI.
pub trait Modal {
    /// Draw the modal content into the given frame.
    fn draw(&self, frame: &mut ratatui::Frame<'_>, area: Rect);

    /// Handle a key event. Returns the action to take.
    fn handle_key(&mut self, key: KeyEvent) -> ModalAction;

    /// Return the title of the modal.
    fn title(&self) -> &str;
}

/// Renders a centered modal frame with a border and title.
///
/// This function:
/// 1. Computes a centered area with clamped width and height for better readability on wide terminals
/// 2. Clears the modal area
/// 3. Renders a `Block` with the given title and border color
/// 4. Returns the inner `Rect` (inside the block borders) for the caller to draw content into
///
/// # Arguments
/// * `frame` - The Ratatui frame to render into
/// * `area` - The full terminal area
/// * `title` - The title text for the modal
/// * `border_color` - The color of the modal border
///
/// # Returns
/// The inner `Rect` where modal content should be drawn
fn modal_block_area(area: Rect) -> Rect {
    let max_usable_w = area.width.saturating_sub(2).max(1);
    let max_usable_h = area.height.saturating_sub(2).max(1);

    let preferred_w = area.width.saturating_sub(area.width / 3);
    let block_w = if max_usable_w >= 54 {
        preferred_w.clamp(54, 108).min(max_usable_w)
    } else {
        max_usable_w
    };

    let preferred_h = area
        .height
        .saturating_sub((area.height / 4).saturating_mul(2));
    let block_h = if max_usable_h >= 10 {
        preferred_h.clamp(10, 28).min(max_usable_h)
    } else {
        max_usable_h
    };

    let block_x = area.x + area.width.saturating_sub(block_w) / 2;
    let block_y = area.y + area.height.saturating_sub(block_h) / 2;
    Rect::new(block_x, block_y, block_w, block_h)
}

pub fn draw_modal_frame(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    title: &str,
    border_color: Color,
) -> Rect {
    let block_area = modal_block_area(area);

    frame.render_widget(Clear, block_area);

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border_color))
        .padding(Padding::new(1, 1, 0, 0))
        .style(Style::default().bg(Color::Rgb(16, 20, 26)));

    let inner = block.inner(block_area);
    frame.render_widget(block, block_area);

    inner
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modal_frame_returns_smaller_inner_rect() {
        let area = Rect::new(0, 0, 80, 24);
        let block_area = modal_block_area(area);

        assert!(block_area.width < area.width);
        assert!(block_area.height < area.height);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .padding(Padding::new(1, 1, 0, 0));
        let inner = block.inner(block_area);

        assert!(inner.width < block_area.width);
        assert!(inner.height < block_area.height);
    }

    #[test]
    fn modal_frame_centering_and_clamp_math() {
        let area_80x24 = modal_block_area(Rect::new(0, 0, 80, 24));
        assert_eq!(area_80x24.width, 54);
        assert_eq!(area_80x24.height, 12);
        assert_eq!(area_80x24.x, 13);
        assert_eq!(area_80x24.y, 6);

        let area_120x40 = modal_block_area(Rect::new(0, 0, 120, 40));
        assert_eq!(area_120x40.width, 80);
        assert_eq!(area_120x40.height, 20);
        assert_eq!(area_120x40.x, 20);
        assert_eq!(area_120x40.y, 10);

        let area_200x44 = modal_block_area(Rect::new(0, 0, 200, 44));
        assert_eq!(area_200x44.width, 108);
        assert_eq!(area_200x44.height, 22);
        assert_eq!(area_200x44.x, 46);
        assert_eq!(area_200x44.y, 11);
    }

    #[test]
    fn modal_action_variants_exist() {
        let _ = ModalAction::Consumed;
        let _ = ModalAction::Dismiss;

        assert_ne!(ModalAction::Consumed, ModalAction::Dismiss);
    }
}
