//! Thin re-export layer — the TUI markdown renderer now lives in the `render` crate.

pub use render::markdown::{
    drain_safe_boundary, render_lines, strip_ansi, Spinner, TerminalRenderer,
};
