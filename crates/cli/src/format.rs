//! Thin re-export layer — formatting functions now live in the `render` crate.

pub(crate) use render::format::{
    format_auto_compaction_notice, format_compact_report, format_cost_report, format_model_report,
    format_model_switch_report, format_status_report, render_config_report, render_export_text,
    render_repl_help, render_version_report, resolve_export_path, status_context, StatusUsage,
    VERSION,
};
