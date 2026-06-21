use api::{CredentialStore, StoredProviderConfig};

use super::mask::mask_secret;
use super::{provider_label, resolve_provider_arg, ProviderChoice};

/// Inspect configured credentials.
///
/// With `check: Some(provider)` this acts as a gate for agents: it returns
/// exit code `0` when the resolved provider has usable credential material in
/// the store and `3` otherwise. No table is printed under `--check` unless
/// `json` is also set. Without `--check` it prints a deterministically-sorted
/// table (human) or `{"active_provider":…,"providers":[…]}` (JSON).
///
/// All secret material is masked via [`mask_secret`]; raw secrets are never
/// written to any stream.
#[must_use]
pub fn run_auth_status(check: Option<&str>, json: bool) -> i32 {
    let store = match api::load_credentials() {
        Ok(store) => store,
        Err(error) => {
            eprintln!("failed to load credentials: {error}");
            return 1;
        }
    };

    if let Some(provider) = check {
        let code = check_exit_code(&store, provider);
        if json {
            println!("{}", render_status(&store, true));
        }
        return code;
    }

    println!("{}", render_status(&store, json));
    0
}

/// List every built-in provider preset, sorted by display name.
///
/// Human format: `  <id>   <display_name>   env: <api_key_env_var or (none)>`.
/// JSON format: an array of `{"id","display_name","env_var"}` objects. Always
/// returns exit code `0`.
#[must_use]
pub fn run_auth_list(json: bool) -> i32 {
    println!("{}", render_list(json));
    0
}

fn check_exit_code(store: &CredentialStore, provider: &str) -> i32 {
    let choice = match resolve_provider_arg(provider) {
        Ok(choice) => choice,
        Err(error) => {
            eprintln!("{error}");
            return 3;
        }
    };
    let configured = store
        .providers
        .get(canonical_provider_id(&choice))
        .is_some_and(has_usable_credential);
    if configured {
        0
    } else {
        3
    }
}

fn canonical_provider_id(choice: &ProviderChoice) -> &str {
    match choice {
        ProviderChoice::Legacy(provider) => provider_label(*provider),
        ProviderChoice::Preset(preset) => preset.id,
    }
}

fn has_usable_credential(config: &StoredProviderConfig) -> bool {
    let api_key_ok = config.api_key.as_deref().is_some_and(|key| !key.is_empty());
    let aws_ok = config
        .aws_secret_access_key
        .as_deref()
        .is_some_and(|secret| !secret.is_empty());
    // `auth_method == "none"` is the custom no-key (local endpoint) case.
    api_key_ok || aws_ok || has_oauth(config) || config.auth_method == "none"
}

fn has_oauth(config: &StoredProviderConfig) -> bool {
    config
        .oauth
        .as_ref()
        .is_some_and(|tokens| !tokens.access_token.is_empty())
}

fn masked_primary_secret(config: &StoredProviderConfig) -> Option<String> {
    if let Some(key) = config.api_key.as_deref().filter(|value| !value.is_empty()) {
        return Some(mask_secret(key));
    }
    if let Some(secret) = config
        .aws_secret_access_key
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        return Some(mask_secret(secret));
    }
    if let Some(tokens) = config.oauth.as_ref() {
        if !tokens.access_token.is_empty() {
            return Some(mask_secret(&tokens.access_token));
        }
    }
    None
}

fn sorted_provider_keys(store: &CredentialStore) -> Vec<&str> {
    let mut keys: Vec<&str> = store.providers.keys().map(String::as_str).collect();
    keys.sort_unstable();
    keys
}

fn sorted_presets() -> Vec<api::ProviderPreset> {
    let mut presets = api::builtin_presets();
    presets.sort_by_key(|preset| preset.display_name);
    presets
}

fn render_status(store: &CredentialStore, json: bool) -> String {
    if json {
        to_pretty(&status_json(store))
    } else {
        render_status_human(store)
    }
}

fn render_status_human(store: &CredentialStore) -> String {
    let mut lines: Vec<String> = Vec::new();
    if let Some(active) = store.active_provider.as_deref() {
        lines.push(format!("active provider: {active}"));
    }
    let keys = sorted_provider_keys(store);
    if keys.is_empty() {
        lines.push("  (no providers configured)".to_string());
    } else {
        for key in keys {
            let config = &store.providers[key];
            let masked = masked_primary_secret(config).unwrap_or_else(|| "(none)".to_string());
            let model = config.default_model.as_deref().unwrap_or("(none)");
            lines.push(format!(
                "  {key}   {auth}   {masked}   model: {model}",
                auth = config.auth_method
            ));
        }
    }
    lines.join("\n")
}

fn status_json(store: &CredentialStore) -> serde_json::Value {
    let providers: Vec<serde_json::Value> = sorted_provider_keys(store)
        .into_iter()
        .map(|key| {
            let config = &store.providers[key];
            serde_json::json!({
                "id": key,
                "auth_method": config.auth_method,
                "default_model": config.default_model,
                "api_key_masked": masked_primary_secret(config).unwrap_or_default(),
                "has_oauth": has_oauth(config),
            })
        })
        .collect();
    serde_json::json!({
        "active_provider": store.active_provider,
        "providers": providers,
    })
}

fn render_list(json: bool) -> String {
    if json {
        return to_pretty(&list_json());
    }
    sorted_presets()
        .into_iter()
        .map(|preset| {
            let env = preset.api_key_env_var.unwrap_or("(none)");
            format!(
                "  {id}   {name}   env: {env}",
                id = preset.id,
                name = preset.display_name
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn list_json() -> serde_json::Value {
    let items: Vec<serde_json::Value> = sorted_presets()
        .into_iter()
        .map(|preset| {
            serde_json::json!({
                "id": preset.id,
                "display_name": preset.display_name,
                "env_var": preset.api_key_env_var,
            })
        })
        .collect();
    serde_json::Value::Array(items)
}

fn to_pretty(value: &serde_json::Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn cfg_with_api_key(key: &str) -> StoredProviderConfig {
        StoredProviderConfig {
            auth_method: "api_key".to_string(),
            api_key: Some(key.to_string()),
            ..Default::default()
        }
    }

    fn store_with(providers: Vec<(&str, StoredProviderConfig)>) -> CredentialStore {
        let mut map = HashMap::new();
        for (id, config) in providers {
            map.insert(id.to_string(), config);
        }
        CredentialStore {
            active_provider: None,
            providers: map,
        }
    }

    #[test]
    fn check_returns_zero_for_configured_provider() {
        let store = store_with(vec![("anthropic", cfg_with_api_key("sk-ant-xyz"))]);
        assert_eq!(check_exit_code(&store, "anthropic"), 0);
    }

    #[test]
    fn check_canonicalizes_alias_before_lookup() {
        let store = store_with(vec![("anthropic", cfg_with_api_key("sk-ant-xyz"))]);
        assert_eq!(check_exit_code(&store, "claude"), 0);
    }

    #[test]
    fn check_returns_three_for_unconfigured_provider() {
        let store = store_with(vec![]);
        assert_eq!(check_exit_code(&store, "groq"), 3);
        assert_eq!(check_exit_code(&store, "openai"), 3);
    }

    #[test]
    fn check_returns_three_for_empty_api_key() {
        let store = store_with(vec![(
            "openai",
            StoredProviderConfig {
                auth_method: "openai_key".to_string(),
                api_key: Some(String::new()),
                ..Default::default()
            },
        )]);
        assert_eq!(check_exit_code(&store, "openai"), 3);
    }

    #[test]
    fn check_returns_three_for_unknown_provider() {
        let store = store_with(vec![]);
        assert_eq!(check_exit_code(&store, "does-not-exist"), 3);
    }

    #[test]
    fn none_auth_method_counts_as_usable() {
        let config = StoredProviderConfig {
            auth_method: "none".to_string(),
            ..Default::default()
        };
        assert!(has_usable_credential(&config));
    }

    #[test]
    fn aws_secret_counts_as_usable() {
        let config = StoredProviderConfig {
            auth_method: "api_key".to_string(),
            aws_secret_access_key: Some("secret".to_string()),
            ..Default::default()
        };
        assert!(has_usable_credential(&config));
    }

    #[test]
    fn oauth_token_counts_as_usable() {
        let config = StoredProviderConfig {
            auth_method: "oauth".to_string(),
            oauth: Some(api::StoredOAuthTokens {
                access_token: "tok".to_string(),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert!(has_usable_credential(&config));
    }

    #[test]
    fn empty_oauth_token_not_usable() {
        let config = StoredProviderConfig {
            auth_method: "oauth".to_string(),
            oauth: Some(api::StoredOAuthTokens::default()),
            ..Default::default()
        };
        assert!(!has_usable_credential(&config));
    }

    #[test]
    fn status_output_never_contains_raw_api_key() {
        let secret = "sk-test-supersecret-body-1234";
        let mut store = store_with(vec![(
            "anthropic",
            StoredProviderConfig {
                auth_method: "api_key".to_string(),
                api_key: Some(secret.to_string()),
                default_model: Some("anthropic/claude-x".to_string()),
                ..Default::default()
            },
        )]);
        store.active_provider = Some("anthropic".to_string());

        let human = render_status(&store, false);
        let json = render_status(&store, true);

        for output in [&human, &json] {
            assert!(!output.contains(secret), "raw secret leaked: {output}");
            assert!(
                !output.contains("supersecret"),
                "secret body leaked: {output}"
            );
            assert!(output.contains("••••1234"), "masked tail missing: {output}");
        }
    }

    #[test]
    fn status_output_never_contains_raw_oauth_token() {
        let token = "oauth-access-token-secret-9999";
        let store = store_with(vec![(
            "copilot",
            StoredProviderConfig {
                auth_method: "oauth".to_string(),
                oauth: Some(api::StoredOAuthTokens {
                    access_token: token.to_string(),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )]);

        let human = render_status(&store, false);
        let json = render_status(&store, true);

        assert!(!human.contains(token));
        assert!(!json.contains(token));
        assert!(human.contains("••••9999"));

        let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid json");
        assert_eq!(parsed["providers"][0]["has_oauth"], serde_json::json!(true));
    }

    #[test]
    fn status_json_reports_active_provider_and_sorted_providers() {
        let mut store = store_with(vec![
            ("openai", cfg_with_api_key("key-two")),
            ("anthropic", cfg_with_api_key("key-one")),
        ]);
        store.active_provider = Some("openai".to_string());

        let json = render_status(&store, true);
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid json");

        assert_eq!(parsed["active_provider"], serde_json::json!("openai"));
        let providers = parsed["providers"].as_array().expect("array");
        assert_eq!(providers.len(), 2);
        assert_eq!(providers[0]["id"], serde_json::json!("anthropic"));
        assert_eq!(providers[1]["id"], serde_json::json!("openai"));
    }

    #[test]
    fn status_json_handles_empty_store() {
        let store = CredentialStore::default();
        let json = render_status(&store, true);
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid json");
        assert_eq!(parsed["active_provider"], serde_json::Value::Null);
        assert_eq!(parsed["providers"].as_array().expect("array").len(), 0);
    }

    #[test]
    fn auth_list_json_includes_all_presets() {
        let json = render_list(true);
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid json");
        let arr = parsed.as_array().expect("array");

        assert_eq!(arr.len(), api::builtin_presets().len());
        assert_eq!(arr.len(), 25);

        for entry in arr {
            assert!(entry.get("id").is_some());
            assert!(entry.get("display_name").is_some());
            assert!(entry.get("env_var").is_some());
        }
    }

    #[test]
    fn auth_list_human_lists_every_preset() {
        let human = render_list(false);
        let lines: Vec<&str> = human.lines().collect();

        assert_eq!(lines.len(), api::builtin_presets().len());
        assert_eq!(lines.len(), 25);
        assert!(human.contains("env: ANTHROPIC_API_KEY"));
        assert!(human.contains("env: (none)"));
    }

    #[test]
    fn run_auth_list_always_returns_zero() {
        assert_eq!(run_auth_list(false), 0);
        assert_eq!(run_auth_list(true), 0);
    }
}
