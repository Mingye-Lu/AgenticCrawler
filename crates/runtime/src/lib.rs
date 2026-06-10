mod compact;
mod config;
mod control;
mod conversation;
mod json;
mod mcp;
pub mod observer;
mod prompt;
mod session;
pub mod settings;
mod summary_compression;
pub mod update_check;
mod usage;

pub use compact::{
    compact_session, estimate_session_tokens, format_compact_summary,
    get_compact_continuation_message, should_compact, CompactionConfig, CompactionResult,
};
pub use config::{
    ConfigEntry, ConfigError, ConfigLoader, ConfigSource, McpClaudeAiProxyServerConfig,
    McpConfigCollection, McpOAuthConfig, McpRemoteServerConfig, McpSdkServerConfig,
    McpServerConfig, McpStdioServerConfig, McpTransport, McpWebSocketServerConfig, OAuthConfig,
    RuntimeConfig, RuntimeFeatureConfig, ACRAWL_SETTINGS_SCHEMA_NAME,
};
pub use control::ControlState;
pub use conversation::{
    auto_compaction_threshold_from_env, ApiClient, ApiRequest, AssistantEvent, AutoCompactionEvent,
    ConversationRuntime, RuntimeError, StaticToolExecutor, ToolError, ToolExecutor, TurnSummary,
};
pub use summary_compression::{
    compress_summary, compress_summary_text, SummaryCompressionBudget, SummaryCompressionResult,
};

pub use mcp::{encode_mcp_frame, read_mcp_frame, McpServerManager, McpTool};
pub use mcp::{mcp_tool_name, mcp_tool_prefix};
pub use mcp::{
    JsonRpcError, JsonRpcId, JsonRpcResponse, ManagedMcpTool, McpServerManagerError,
    McpToolCallContent, McpToolCallParams, McpToolCallResult, UnsupportedMcpServer,
};
pub use observer::RuntimeObserver;
pub use prompt::{prepend_bullets, PromptBuildError, SystemPromptBuilder};
pub use session::{
    ChildSession, ContentBlock, ConversationMessage, MessageRole, Session, SessionError,
};
pub use settings::{
    config_home_dir, load_settings, resolve_output_dir, save_settings, settings_file_path,
    settings_get_action_cache_ttl_secs, settings_get_action_caching,
    settings_get_auto_compact_tokens, settings_get_compaction_llm_summarization,
    settings_get_compaction_max_summary_chars,
    settings_get_compaction_preserve_recent_messages_floor,
    settings_get_compaction_preserve_recent_tokens, settings_get_compaction_prune_max_output_chars,
    settings_get_compaction_prune_protect_tokens, settings_get_confidence_tracking,
    settings_get_fork_child_max_steps, settings_get_fork_wait_timeout_secs, settings_get_headless,
    settings_get_html_diff_mode, settings_get_loop_detection, settings_get_loop_detection_window,
    settings_get_loop_nudge_threshold, settings_get_max_concurrent_per_parent,
    settings_get_max_fork_depth, settings_get_max_steps, settings_get_max_total_agents,
    settings_get_output_dir, settings_get_page_fingerprinting, settings_get_planning_interval,
    settings_get_self_healing, settings_get_self_healing_max_retries, update_settings, Settings,
};
pub use update_check::{check_for_update, check_for_update_force, UpdateInfo};
pub use usage::{
    estimate_cost_usd, estimate_cost_usd_with_pricing, format_usd, pricing_for_model,
    summary_lines, summary_lines_for_model, ModelPricing, TokenUsage, UsageCostEstimate,
    UsageTracker,
};

#[cfg(test)]
pub(crate) fn test_env_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
