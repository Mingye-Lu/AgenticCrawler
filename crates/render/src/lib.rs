pub mod format;
pub mod markdown;
pub mod sink;
pub mod tool_format;

#[allow(deprecated)]
pub use format::{
    default_export_filename, format_auto_compaction_notice, format_compact_report,
    format_cost_report, format_model_report, format_model_switch_report, format_status_report,
    render_config_report, render_export_text, render_repl_help, render_version_report,
    resolve_export_path, status_context, truncate_for_summary, StatusContext, StatusUsage,
    BUILD_DATE, BUILD_TARGET, GIT_SHA, VERSION,
};
pub use markdown::{
    drain_safe_boundary, render_lines, strip_ansi, text_to_ansi, ColorTheme, MarkdownStreamState,
    Spinner, TerminalRenderer,
};
pub use sink::{OutputSink, StdoutSink};
pub use tool_format::{
    format_tool_error_line, format_tool_start_line, format_tool_success_line, tool_input_summary,
    truncate_with_ellipsis, ToolLine,
};
