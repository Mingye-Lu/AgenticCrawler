use std::io::{self, Write};

use super::{persist_provider_credentials, Provider};

pub(super) fn run_auth() -> Result<(), Box<dyn std::error::Error>> {
    let existing = api::load_credentials()
        .unwrap_or_default()
        .providers
        .get("other")
        .cloned()
        .unwrap_or_default();
    eprint!(
        "Base URL [{}]: ",
        existing
            .base_url
            .as_deref()
            .unwrap_or("http://localhost:11434/v1")
    );
    io::stderr().flush()?;
    let mut base_url = String::new();
    io::stdin().read_line(&mut base_url)?;
    let base_url = match base_url.trim() {
        "" => existing
            .base_url
            .clone()
            .unwrap_or_else(|| "http://localhost:11434/v1".to_string()),
        value => value.to_string(),
    };

    eprint!("API key (optional, press Enter to skip): ");
    io::stderr().flush()?;
    let mut key = String::new();
    io::stdin().read_line(&mut key)?;
    let key = key.trim().to_string();

    persist_provider_credentials(
        Provider::Other,
        api::StoredProviderConfig {
            auth_method: if key.is_empty() {
                "none".to_string()
            } else {
                "api_key".to_string()
            },
            api_key: (!key.is_empty()).then_some(key),
            base_url: Some(base_url),
            default_model: existing.default_model,
            ..Default::default()
        },
    )
}
