#[allow(
    clippy::format_push_string,
    clippy::redundant_closure_for_method_calls,
    clippy::map_unwrap_or
)]
pub mod events;
pub mod modal;
pub mod repl_app;
pub mod tool_panel;

pub use events::ReplTuiEvent;
pub use repl_app::run_repl_ratatui;
