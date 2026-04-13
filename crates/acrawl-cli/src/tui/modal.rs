//! Base modal trait and shared rendering helpers for the Ratatui TUI modal system.

use crossterm::event::KeyEvent;
use ratatui::layout::{Margin, Rect};
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, Borders, Clear};

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
/// 1. Computes a centered area using `Margin { horizontal: area.width / 6, vertical: area.height / 4 }`
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
pub fn draw_modal_frame(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    title: &str,
    border_color: Color,
) -> Rect {
    let block_area = area.inner(Margin {
        horizontal: area.width / 6,
        vertical: area.height / 4,
    });

    frame.render_widget(Clear, block_area);

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));

    let inner = block.inner(block_area);
    frame.render_widget(block, block_area);

    inner
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modal_frame_returns_smaller_inner_rect() {
        // Test that the inner rect is strictly smaller than the outer area
        let area = Rect::new(0, 0, 80, 24);
        let block_area = area.inner(Margin {
            horizontal: area.width / 6,
            vertical: area.height / 4,
        });

        // The block_area should be smaller than the original area
        assert!(block_area.width < area.width);
        assert!(block_area.height < area.height);

        // The inner rect (inside the block borders) should be even smaller
        let block = Block::default().borders(Borders::ALL);
        let inner = block.inner(block_area);

        assert!(inner.width < block_area.width);
        assert!(inner.height < block_area.height);
    }

    #[test]
    fn modal_frame_centering_math() {
        // Test margin computation for 80x24
        let area_80x24 = Rect::new(0, 0, 80, 24);
        let block_area_80x24 = area_80x24.inner(Margin {
            horizontal: area_80x24.width / 6,
            vertical: area_80x24.height / 4,
        });

        // Margin should be width/6 = 80/6 ≈ 13, height/4 = 24/4 = 6
        // So block_area should be 80 - 2*13 = 54 wide, 24 - 2*6 = 12 tall
        assert_eq!(block_area_80x24.width, 54);
        assert_eq!(block_area_80x24.height, 12);

        // Test margin computation for 120x40
        let area_120x40 = Rect::new(0, 0, 120, 40);
        let block_area_120x40 = area_120x40.inner(Margin {
            horizontal: area_120x40.width / 6,
            vertical: area_120x40.height / 4,
        });

        // Margin should be width/6 = 120/6 = 20, height/4 = 40/4 = 10
        // So block_area should be 120 - 2*20 = 80 wide, 40 - 2*10 = 20 tall
        assert_eq!(block_area_120x40.width, 80);
        assert_eq!(block_area_120x40.height, 20);
    }

    #[test]
    fn modal_action_variants_exist() {
        let _ = ModalAction::Consumed;
        let _ = ModalAction::Dismiss;

        assert_ne!(ModalAction::Consumed, ModalAction::Dismiss);
    }
}
