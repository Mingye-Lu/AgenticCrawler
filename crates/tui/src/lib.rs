//! TUI module for acrawl — Ratatui-based interactive terminal interface.
//!
//! This crate provides the full terminal UI for the acrawl interactive REPL,
//! including modals, rendering, and event handling.

pub use acrawl_ui::{app, auth, display_width, error, events, output_sink, session_mgr};
pub use acrawl_ui::{CliOutputFormat, TOKIO_RUNTIME};

// ── Thin re-export shims (provide crate::format, crate::markdown, crate::tool_format) ──

/// Re-exports from the `render` crate so `crate::format::*` paths resolve.
#[allow(unused_imports, deprecated)]
pub(crate) mod format {
    pub(crate) use render::format::{
        default_export_filename, format_auto_compaction_notice, format_compact_report,
        format_cost_report, format_model_report, format_model_switch_report, format_status_report,
        render_config_report, render_export_text, render_repl_help, render_version_report,
        resolve_export_path, status_context, truncate_for_summary, StatusContext, StatusUsage,
        VERSION,
    };
}

/// Re-exports from the `render` crate so `crate::markdown::*` paths resolve.
#[allow(unused_imports)]
pub(crate) mod markdown {
    pub use render::markdown::{
        drain_safe_boundary, render_lines, strip_ansi, text_to_ansi, ColorTheme,
        MarkdownStreamState, Spinner, TerminalRenderer,
    };
}

/// Re-exports from the `render` crate so `crate::tool_format::*` paths resolve.
#[allow(unused_imports)]
pub(crate) mod tool_format {
    pub(crate) use render::tool_format::{
        format_tool_error_line, format_tool_start_line, format_tool_success_line,
        tool_input_summary, truncate_with_ellipsis, ToolLine,
    };
}

// ── TUI utility modules (moved from CLI in T30) ──

pub(crate) mod active_modal;
pub mod child_tabs;
pub(crate) mod modal;
pub(crate) mod model_list;
pub mod tool_pairing;

pub mod modals;

// Crate-root re-exports for backward compatibility paths
pub(crate) use modals::auth as auth_modal;
pub(crate) use modals::grouped_model_list;
pub(crate) use modals::model as model_modal;
pub(crate) use modals::session as session_modal;

// ── The moved REPL files (source of truth now lives here) ──

pub mod repl_app;
pub mod repl_render;

// ── Compatibility module: provides `crate::tui::*` paths that the moved files expect ──

#[allow(unused_imports)]
pub(crate) mod tui {
    pub(crate) use crate::active_modal;
    pub(crate) use crate::auth_modal;
    pub(crate) use crate::child_tabs;
    pub(crate) use crate::grouped_model_list;
    pub(crate) use crate::modal;
    pub(crate) use crate::model_list;
    pub(crate) use crate::model_modal;
    pub(crate) use crate::repl_app;
    pub(crate) use crate::repl_app::run_repl_ratatui;
    pub(crate) use crate::repl_render;
    pub(crate) use crate::session_modal;
    pub(crate) use acrawl_ui::events;
    pub(crate) use acrawl_ui::events::ReplTuiEvent;
}

// ── Public entry point ──

/// Run the TUI REPL. Delegates to `repl_app::run_repl_ratatui`.
///
/// Initializes the crate-level `TOKIO_RUNTIME` if not already set so that all
/// async helpers (`block_on_runtime_future`, auth modal spawns, etc.) can
/// resolve `crate::TOKIO_RUNTIME` successfully.
pub fn run_tui(
    model: String,
    allowed_tools: Option<std::collections::BTreeSet<String>>,
) -> Result<(), Box<dyn std::error::Error>> {
    TOKIO_RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("failed to create tokio runtime")
    });
    repl_app::run_repl_ratatui(model, allowed_tools)
}
