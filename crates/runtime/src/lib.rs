mod bash;
mod bootstrap;
mod compact;
mod config;
mod conversation;
mod file_ops;
mod hooks;
mod json;
mod mcp;
mod mcp_client;
mod mcp_stdio;
mod mcp_types;
mod oauth;
pub mod observer;
mod prompt;
mod remote;
pub mod sandbox;
mod session;
pub mod settings;
mod usage;

pub use bash::{execute_bash, BashCommandInput, BashCommandOutput};
pub use bootstrap::{BootstrapPhase, BootstrapPlan};
pub use compact::{
    compact_session, estimate_session_tokens, format_compact_summary,
    get_compact_continuation_message, should_compact, CompactionConfig, CompactionResult,
};
pub use config::{
    ConfigEntry, ConfigError, ConfigLoader, ConfigSource, McpClaudeAiProxyServerConfig,
    McpConfigCollection, McpOAuthConfig, McpRemoteServerConfig, McpSdkServerConfig,
    McpServerConfig, McpStdioServerConfig, McpTransport, McpWebSocketServerConfig, OAuthConfig,
    RuntimeConfig, RuntimeFeatureConfig, RuntimeHookConfig, ScopedMcpServerConfig,
    ACRAWL_SETTINGS_SCHEMA_NAME,
};
pub use conversation::{
    auto_compaction_threshold_from_env, ApiClient, ApiRequest, AssistantEvent, AutoCompactionEvent,
    ConversationRuntime, RuntimeError, StaticToolExecutor, ToolError, ToolExecutor, TurnSummary,
};
pub use file_ops::{
    edit_file, glob_search, grep_search, read_file, write_file, EditFileOutput, GlobSearchOutput,
    GrepSearchInput, GrepSearchOutput, ReadFileOutput, StructuredPatchHunk, TextFilePayload,
    WriteFileOutput,
};
pub use hooks::{HookEvent, HookRunResult, HookRunner};
pub use mcp::{
    mcp_server_signature, mcp_tool_name, mcp_tool_prefix, normalize_name_for_mcp,
    scoped_mcp_config_hash, unwrap_ccr_proxy_url,
};
pub use mcp_client::{
    McpClaudeAiProxyTransport, McpClientAuth, McpClientBootstrap, McpClientTransport,
    McpRemoteTransport, McpSdkTransport, McpStdioTransport,
};
pub use mcp_stdio::{spawn_mcp_stdio_process, McpServerManager, McpStdioProcess};
pub use mcp_types::{
    JsonRpcError, JsonRpcId, JsonRpcNotification, JsonRpcRequest, JsonRpcResponse, ManagedMcpTool,
    McpInitializeClientInfo, McpInitializeParams, McpInitializeResult, McpInitializeServerInfo,
    McpListResourcesParams, McpListResourcesResult, McpListToolsParams, McpListToolsResult,
    McpReadResourceParams, McpReadResourceResult, McpResource, McpResourceContents,
    McpServerManagerError, McpTool, McpToolCallContent, McpToolCallParams, McpToolCallResult,
    UnsupportedMcpServer,
};
pub use oauth::{
    clear_oauth_credentials, code_challenge_s256, credentials_path, generate_pkce_pair,
    generate_state, load_oauth_credentials, loopback_redirect_uri, parse_oauth_callback_query,
    parse_oauth_callback_request_target, save_oauth_credentials, OAuthAuthorizationRequest,
    OAuthCallbackParams, OAuthRefreshRequest, OAuthTokenExchangeRequest, OAuthTokenSet,
    PkceChallengeMethod, PkceCodePair,
};
pub use observer::RuntimeObserver;
pub use prompt::{
    load_system_prompt, prepend_bullets, ProjectContext, PromptBuildError, SystemPromptBuilder,
    FRONTIER_MODEL_NAME, SYSTEM_PROMPT_DYNAMIC_BOUNDARY,
};
pub use remote::{
    inherited_upstream_proxy_env, no_proxy_list, read_token, upstream_proxy_ws_url,
    RemoteSessionContext, UpstreamProxyBootstrap, UpstreamProxyState, DEFAULT_REMOTE_BASE_URL,
    DEFAULT_SESSION_TOKEN_PATH, DEFAULT_SYSTEM_CA_BUNDLE, NO_PROXY_HOSTS, UPSTREAM_PROXY_ENV_KEYS,
};
pub use session::{ContentBlock, ConversationMessage, MessageRole, Session, SessionError};
pub use settings::{
    config_home_dir, load_settings, save_settings, settings_file_path,
    settings_get_auto_compact_tokens, settings_get_fork_child_max_steps,
    settings_get_fork_wait_timeout_secs, settings_get_headless,
    settings_get_max_concurrent_per_parent, settings_get_max_fork_depth, settings_get_max_steps,
    settings_get_max_total_agents, settings_get_workspace_dir, update_settings, Settings,
};
pub use usage::{
    format_usd, pricing_for_model, ModelPricing, TokenUsage, UsageCostEstimate, UsageTracker,
};

#[cfg(test)]
pub(crate) fn test_env_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
