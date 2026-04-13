mod client;
pub mod provider_types;
pub mod codex;
pub mod credentials;
mod error;
pub mod models;
pub mod openai;
pub mod responses;
mod sse;
mod types;

pub use client::{
    oauth_token_is_expired, read_base_url, resolve_saved_oauth_token, resolve_startup_auth_source,
    AnthropicClient, AuthSource, MessageStream, OAuthTokenSet,
};
pub use codex::{
    codex_oauth_config, codex_redirect_uri, login as codex_login, logout as codex_logout,
    read_codex_model, read_codex_responses_url, resolve_codex_auth, save_codex_credentials,
    CodexClient, CodexLoginRequest, CodexMessageStream, CODEX_CALLBACK_PORT, CODEX_SCOPES,
    DEFAULT_CODEX_MODEL, OPENAI_AUTH_URL, OPENAI_CLIENT_ID, OPENAI_TOKEN_URL,
};
pub use credentials::{
    CredentialError, CredentialStore, StoredOAuthTokens, StoredProviderConfig,
    credentials_file_path, get_active_config, load_credentials, load_credentials_from_path,
    remove_provider_config, save_credentials, save_credentials_to_path, set_provider_config,
};
pub use error::ApiError;
pub use models::{
    AnthropicModel, AnthropicModelList, OpenAiModel, OpenAiModelList, list_anthropic_models,
    list_openai_models,
};
pub use openai::{OpenAiClient, OpenAiMessageStream, DEFAULT_OPENAI_MODEL};
pub use sse::{parse_frame, SseParser};
pub use responses::{
    ResponsesMessageStream, ResponsesStreamState, build_responses_request,
    convert_responses_messages, convert_responses_tool, responses_tool_choice,
};
pub use provider_types::{AuthMethod, Provider, ProviderConfig};
pub use types::{
    ContentBlockDelta, ContentBlockDeltaEvent, ContentBlockStartEvent, ContentBlockStopEvent,
    InputContentBlock, InputMessage, MessageDelta, MessageDeltaEvent, MessageRequest,
    MessageResponse, MessageStartEvent, MessageStopEvent, OutputContentBlock, StreamEvent,
    ToolChoice, ToolDefinition, ToolResultContentBlock, Usage,
};

#[cfg(test)]
pub(crate) fn test_env_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
