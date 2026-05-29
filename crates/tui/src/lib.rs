//! TUI module for acrawl — Ratatui-based interactive terminal interface.
//!
//! This crate provides the full terminal UI for the acrawl interactive REPL,
//! including modals, rendering, and event handling.

use std::sync::OnceLock;

/// Tokio runtime handle — mirrors the static in the CLI binary.  Populated by
/// the binary's `main()` before calling into TUI code.
pub static TOKIO_RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

// ── Thin re-export shims (provide crate::format, crate::markdown, crate::tool_format) ──

/// Re-exports from the `render` crate so `crate::format::*` paths resolve.
#[allow(unused_imports, deprecated)]
pub(crate) mod format {
    pub(crate) use render::format::{
        default_export_filename, format_auto_compaction_notice, format_compact_report,
        format_cost_report, format_model_report, format_model_switch_report, format_status_report,
        render_config_report, render_export_text, render_repl_help, render_version_report,
        resolve_export_path, status_context, truncate_for_summary, StatusContext, StatusUsage,
        DEFAULT_DATE, VERSION,
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

// ── Utility modules included from CLI (will eventually move here properly) ──

#[path = "../../cli/src/display_width.rs"]
pub(crate) mod display_width;

#[allow(dead_code)]
#[path = "../../cli/src/error.rs"]
pub(crate) mod error;

#[allow(dead_code)]
#[path = "../../cli/src/output_sink.rs"]
pub(crate) mod output_sink;

#[allow(dead_code)]
#[path = "../../cli/src/session_mgr.rs"]
pub(crate) mod session_mgr;

// ── Auth and App modules (included from CLI via #[path]) ──

#[allow(dead_code, unused_imports, deprecated)]
#[path = "../../cli/src/auth/mod.rs"]
pub(crate) mod auth;

#[allow(dead_code, unused_imports, deprecated)]
#[path = "../../cli/src/app/mod.rs"]
pub(crate) mod app;

// Re-export Provider at crate root (auth/mod.rs uses `super::Provider`)
pub(crate) use app::Provider;

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CliOutputFormat {
    Text,
    Json,
}

impl CliOutputFormat {
    #[allow(dead_code)]
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "text" => Ok(Self::Text),
            "json" => Ok(Self::Json),
            other => Err(format!(
                "unsupported value for --output-format: {other} (expected text or json)"
            )),
        }
    }
}

// ── TUI utility modules (moved from CLI in T30) ──

pub(crate) mod active_modal;
pub mod child_tabs;
pub mod events;
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
    pub(crate) use crate::events;
    pub(crate) use crate::events::ReplTuiEvent;
    pub(crate) use crate::grouped_model_list;
    pub(crate) use crate::modal;
    pub(crate) use crate::model_list;
    pub(crate) use crate::model_modal;
    pub(crate) use crate::repl_app;
    pub(crate) use crate::repl_app::run_repl_ratatui;
    pub(crate) use crate::repl_render;
    pub(crate) use crate::session_modal;
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
