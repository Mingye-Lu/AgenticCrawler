use super::{
    builder::{build_provider_config, CredGroup, CredInputs},
    group::{flag_group_for, is_scriptable},
    persist_preset_credentials, persist_provider_credentials, provider_choice_label,
    provider_label, resolve_provider_arg, ProviderChoice,
};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AuthFlags {
    pub api_key: Option<String>,
    pub access_key: Option<String>,
    pub secret_key: Option<String>,
    pub region: Option<String>,
    pub resource_name: Option<String>,
    pub deployment_name: Option<String>,
    pub base_url: Option<String>,
    pub gcp_project: Option<String>,
    pub gcp_region: Option<String>,
}

#[must_use]
pub fn run_auth_configure(provider: &str, flags: AuthFlags, json: bool) -> i32 {
    let choice = match resolve_provider_arg(provider) {
        Ok(choice) => choice,
        Err(error) => {
            eprintln!("{error}");
            return 2;
        }
    };

    let canonical_id = canonical_provider_id(&choice);
    if !is_scriptable(canonical_id) {
        eprintln!(
            "provider `{canonical_id}` is not scriptable non-interactively; run `acrawl auth {canonical_id}` to authenticate via interactive flow"
        );
        return 2;
    }

    let Some(group) = flag_group_for(canonical_id) else {
        eprintln!("provider `{canonical_id}` does not support non-interactive configuration");
        return 2;
    };

    let inputs = match validated_inputs(group, flags) {
        Ok(inputs) => inputs,
        Err(flag) => {
            eprintln!("missing required flag --{flag}");
            return 2;
        }
    };

    let inputs = preset_backfilled_inputs(group, &choice, inputs);

    let config = build_provider_config(group, inputs);
    let auth_method = config.auth_method.clone();
    let persist_result = match choice {
        ProviderChoice::Legacy(provider) => persist_provider_credentials(provider, config),
        ProviderChoice::Preset(preset) => persist_preset_credentials(preset.id, config),
    };

    if let Err(error) = persist_result {
        eprintln!("{error}");
        return 1;
    }

    if json {
        println!(
            "{}",
            serde_json::json!({
                "ok": true,
                "provider": canonical_id,
                "auth_method": auth_method,
            })
        );
    } else {
        eprintln!("✓ {} credentials configured.", display_name(&choice));
    }

    0
}

fn canonical_provider_id(choice: &ProviderChoice) -> &str {
    match choice {
        ProviderChoice::Legacy(provider) => provider_label(*provider),
        ProviderChoice::Preset(preset) => preset.id,
    }
}

fn display_name(choice: &ProviderChoice) -> &str {
    match choice {
        ProviderChoice::Legacy(_) => provider_choice_label(choice),
        ProviderChoice::Preset(preset) => preset.display_name,
    }
}

fn preset_backfilled_inputs(
    group: CredGroup,
    choice: &ProviderChoice,
    mut inputs: CredInputs,
) -> CredInputs {
    if let (CredGroup::Simple, ProviderChoice::Preset(preset)) = (group, choice) {
        inputs.base_url = Some(preset.base_url.to_string());
    }
    inputs
}

fn validated_inputs(group: CredGroup, flags: AuthFlags) -> Result<CredInputs, &'static str> {
    let inputs = CredInputs {
        api_key: normalized(flags.api_key),
        access_key: normalized(flags.access_key),
        secret_key: normalized(flags.secret_key),
        region: normalized(flags.region),
        resource_name: normalized(flags.resource_name),
        deployment_name: normalized(flags.deployment_name),
        base_url: normalized(flags.base_url),
        gcp_project: normalized(flags.gcp_project),
        gcp_region: normalized(flags.gcp_region),
    };

    match group {
        CredGroup::Simple => {
            require(inputs.api_key.as_deref(), "api-key")?;
            Ok(inputs)
        }
        CredGroup::Bedrock => {
            require(inputs.access_key.as_deref(), "access-key")?;
            require(inputs.secret_key.as_deref(), "secret-key")?;
            Ok(CredInputs {
                region: Some(inputs.region.unwrap_or_else(|| "us-east-1".to_string())),
                ..inputs
            })
        }
        CredGroup::Azure => {
            require(inputs.resource_name.as_deref(), "resource-name")?;
            require(inputs.deployment_name.as_deref(), "deployment-name")?;
            require(inputs.api_key.as_deref(), "api-key")?;
            Ok(inputs)
        }
        CredGroup::Custom => {
            require(inputs.base_url.as_deref(), "base-url")?;
            Ok(inputs)
        }
        CredGroup::Vertex => {
            require(inputs.gcp_project.as_deref(), "gcp-project")?;
            require(inputs.gcp_region.as_deref(), "gcp-region")?;
            require(inputs.api_key.as_deref(), "api-key")?;
            Ok(inputs)
        }
    }
}

fn normalized(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let trimmed = value.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    })
}

fn require(value: Option<&str>, flag: &'static str) -> Result<(), &'static str> {
    if value.is_some() {
        Ok(())
    } else {
        Err(flag)
    }
}

#[cfg(test)]
mod tests {
    use super::{run_auth_configure, AuthFlags};
    use api::load_credentials;
    use std::fs;
    use std::sync::{Mutex, OnceLock};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn test_env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn with_clean_config_env<T>(f: impl FnOnce() -> T) -> T {
        let _guard = test_env_lock();
        let saved_config_home = std::env::var_os("ACRAWL_CONFIG_HOME");
        let temp_dir = std::env::temp_dir().join(format!(
            "auth-configure-tests-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time should be after epoch")
                .as_nanos()
        ));
        fs::create_dir_all(&temp_dir).expect("create temp config home");
        std::env::set_var("ACRAWL_CONFIG_HOME", &temp_dir);
        let result = f();
        match saved_config_home {
            Some(value) => std::env::set_var("ACRAWL_CONFIG_HOME", value),
            None => std::env::remove_var("ACRAWL_CONFIG_HOME"),
        }
        fs::remove_dir_all(temp_dir).expect("cleanup temp config home");
        result
    }

    #[test]
    fn openai_happy_path_stores_expected_shape() {
        with_clean_config_env(|| {
            let exit_code = run_auth_configure(
                "openai",
                AuthFlags {
                    api_key: Some("sk-test-123".to_string()),
                    ..AuthFlags::default()
                },
                false,
            );

            assert_eq!(exit_code, 0);

            let store = load_credentials().expect("load credentials");
            let config = store
                .providers
                .get("openai")
                .expect("openai config should be stored");
            assert_eq!(config.auth_method, "openai_key");
            assert_eq!(config.api_key.as_deref(), Some("sk-test-123"));
            assert_eq!(config.base_url, None);
        });
    }

    #[test]
    fn missing_required_flag_returns_usage_error() {
        with_clean_config_env(|| {
            let exit_code = run_auth_configure("azure", AuthFlags::default(), false);
            assert_eq!(exit_code, 2);

            let store = load_credentials().expect("load credentials");
            assert!(store.providers.is_empty());
        });
    }

    #[test]
    fn copilot_returns_usage_error() {
        with_clean_config_env(|| {
            let exit_code = run_auth_configure(
                "copilot",
                AuthFlags {
                    api_key: Some("ignored".to_string()),
                    ..AuthFlags::default()
                },
                false,
            );
            assert_eq!(exit_code, 2);

            let store = load_credentials().expect("load credentials");
            assert!(store.providers.is_empty());
        });
    }

    #[test]
    fn bedrock_defaults_region_when_omitted() {
        with_clean_config_env(|| {
            let exit_code = run_auth_configure(
                "amazon-bedrock",
                AuthFlags {
                    access_key: Some("AKIA_TEST".to_string()),
                    secret_key: Some("secret-test".to_string()),
                    ..AuthFlags::default()
                },
                false,
            );

            assert_eq!(exit_code, 0);

            let store = load_credentials().expect("load credentials");
            let config = store
                .providers
                .get("amazon-bedrock")
                .expect("bedrock config should be stored");
            assert_eq!(config.region.as_deref(), Some("us-east-1"));
            assert_eq!(config.api_key.as_deref(), Some("AKIA_TEST"));
            assert_eq!(config.aws_secret_access_key.as_deref(), Some("secret-test"));
        });
    }

    #[test]
    fn legacy_alias_persists_under_canonical_provider_key() {
        with_clean_config_env(|| {
            let exit_code = run_auth_configure(
                "gpt",
                AuthFlags {
                    api_key: Some("sk-test-456".to_string()),
                    ..AuthFlags::default()
                },
                false,
            );

            assert_eq!(exit_code, 0);

            let store = load_credentials().expect("load credentials");
            assert!(store.providers.contains_key("openai"));
            assert!(!store.providers.contains_key("gpt"));
        });
    }

    #[test]
    fn preset_provider_persists_under_preset_id() {
        with_clean_config_env(|| {
            let exit_code = run_auth_configure(
                "groq",
                AuthFlags {
                    api_key: Some("groq-key".to_string()),
                    ..AuthFlags::default()
                },
                false,
            );

            assert_eq!(exit_code, 0);

            let store = load_credentials().expect("load credentials");
            let config = store
                .providers
                .get("groq")
                .expect("groq config should be stored");
            assert_eq!(config.auth_method, "api_key");
            assert_eq!(
                config.base_url.as_deref(),
                Some("https://api.groq.com/openai/v1")
            );
        });
    }

    #[test]
    fn custom_requires_base_url_but_api_key_is_optional() {
        with_clean_config_env(|| {
            let exit_code = run_auth_configure(
                "other",
                AuthFlags {
                    base_url: Some("http://localhost:11434/v1".to_string()),
                    ..AuthFlags::default()
                },
                false,
            );

            assert_eq!(exit_code, 0);

            let store = load_credentials().expect("load credentials");
            let config = store
                .providers
                .get("other")
                .expect("other config should be stored");
            assert_eq!(config.auth_method, "none");
            assert_eq!(
                config.base_url.as_deref(),
                Some("http://localhost:11434/v1")
            );
            assert_eq!(config.api_key, None);
        });
    }

    #[test]
    fn unknown_provider_returns_usage_error() {
        with_clean_config_env(|| {
            let exit_code = run_auth_configure("does-not-exist", AuthFlags::default(), false);
            assert_eq!(exit_code, 2);
        });
    }

    #[test]
    fn vertex_requires_project_region_and_api_key() {
        with_clean_config_env(|| {
            let exit_code = run_auth_configure(
                "vertex",
                AuthFlags {
                    api_key: Some("ya29.test-token".to_string()),
                    gcp_project: Some("my-project".to_string()),
                    gcp_region: Some("us-central1".to_string()),
                    ..AuthFlags::default()
                },
                false,
            );

            assert_eq!(exit_code, 0);

            let store = load_credentials().expect("load credentials");
            let config = store
                .providers
                .get("vertex")
                .expect("vertex config should be stored");
            assert_eq!(config.auth_method, "api_key");
            assert_eq!(config.gcp_project_id.as_deref(), Some("my-project"));
            assert_eq!(config.gcp_region.as_deref(), Some("us-central1"));
            assert_eq!(config.api_key.as_deref(), Some("ya29.test-token"));
        });
    }

    #[test]
    fn anthropic_alias_uses_canonical_key() {
        with_clean_config_env(|| {
            let exit_code = run_auth_configure(
                "claude",
                AuthFlags {
                    api_key: Some("sk-ant-test-123".to_string()),
                    ..AuthFlags::default()
                },
                false,
            );

            assert_eq!(exit_code, 0);

            let store = load_credentials().expect("load credentials");
            let config = store
                .providers
                .get("anthropic")
                .expect("anthropic config should be stored");
            assert_eq!(config.auth_method, "api_key");
            assert!(!store.providers.contains_key("claude"));
        });
    }
}
