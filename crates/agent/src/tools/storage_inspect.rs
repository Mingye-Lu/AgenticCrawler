use serde_json::{json, Value};

use browser::StorageType;

use crate::BrowserContext;
use crate::{ToolEffect, ToolExecutionError};

pub async fn inspect_cookies(
    input: &Value,
    browser: &mut BrowserContext,
) -> Result<ToolEffect, ToolExecutionError> {
    let domain_filter = input.get("domain").and_then(Value::as_str);
    let issues_only = input
        .get("issues_only")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let cookies = browser
        .acquire_bridge()
        .await
        .map_err(|e| ToolExecutionError::new(e.to_string()))?
        .get_cookies()
        .await
        .map_err(|e| ToolExecutionError::new(e.to_string()))?;

    let current_url = browser.snapshot_url().unwrap_or("").to_string();
    let current_domain = extract_domain(&current_url);

    let mut analyzed_cookies = Vec::new();
    let mut total_count = 0;
    let mut with_issues_count = 0;
    let mut third_party_count = 0;
    let mut session_count = 0;
    let mut persistent_count = 0;

    for cookie in cookies {
        total_count += 1;

        let mut issues = Vec::new();

        if !cookie.secure {
            issues.push("missing_secure".to_string());
        }

        if !cookie.http_only {
            issues.push("missing_httponly".to_string());
        }

        if let Some(same_site) = &cookie.same_site {
            if same_site == "None" && !cookie.secure {
                issues.push("sameSite_none_without_secure".to_string());
            }
        }

        if let Some(expires) = cookie.expires {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0.0, |d| d.as_secs_f64());
            let lifetime_secs = expires - now;
            let thirteen_months_secs = 13.0 * 30.44 * 24.0 * 3600.0;
            if lifetime_secs > thirteen_months_secs {
                issues.push("excessive_lifetime".to_string());
            }
            persistent_count += 1;
        } else {
            session_count += 1;
        }

        if cookie.domain.starts_with('.') {
            let dot_count = cookie.domain.matches('.').count();
            if dot_count < 2 {
                issues.push("overly_broad_domain".to_string());
            }
        }

        let is_third_party = !current_domain.is_empty()
            && !cookie.domain.is_empty()
            && !current_domain.contains(&cookie.domain)
            && !cookie.domain.contains(&current_domain);

        if is_third_party {
            third_party_count += 1;
        }

        if !issues.is_empty() {
            with_issues_count += 1;
        }

        if let Some(domain_filter) = domain_filter {
            if !cookie.domain.contains(domain_filter) {
                continue;
            }
        }

        if issues_only && issues.is_empty() {
            continue;
        }

        analyzed_cookies.push(json!({
            "name": cookie.name,
            "value": cookie.value,
            "domain": cookie.domain,
            "path": cookie.path,
            "expires": cookie.expires,
            "secure": cookie.secure,
            "http_only": cookie.http_only,
            "same_site": cookie.same_site,
            "size_bytes": cookie.size_bytes,
            "issues": issues,
            "third_party": is_third_party,
        }));
    }

    let result = json!({
        "cookies": analyzed_cookies,
        "summary": {
            "total": total_count,
            "with_issues": with_issues_count,
            "third_party": third_party_count,
            "session": session_count,
            "persistent": persistent_count,
        }
    });

    Ok(ToolEffect::reply_json(&result))
}

pub async fn inspect_storage(
    input: &Value,
    browser: &mut BrowserContext,
) -> Result<ToolEffect, ToolExecutionError> {
    let target = input.get("target").and_then(Value::as_str).unwrap_or("all");
    let pattern = input.get("pattern").and_then(Value::as_str);

    let storage_type = match target {
        "local" => StorageType::Local,
        "session" => StorageType::Session,
        _ => StorageType::All,
    };

    let storage = browser
        .acquire_bridge()
        .await
        .map_err(|e| ToolExecutionError::new(e.to_string()))?
        .get_storage(storage_type)
        .await
        .map_err(|e| ToolExecutionError::new(e.to_string()))?;

    let mut local_storage = Vec::new();
    let mut session_storage = Vec::new();
    let mut total_size_bytes = 0;

    for entry in storage.0 {
        if let Some(pattern) = pattern {
            if !entry.key.contains(pattern) {
                continue;
            }
        }
        total_size_bytes += entry.size_bytes;
        local_storage.push(json!({
            "key": entry.key,
            "value": entry.value,
            "size_bytes": entry.size_bytes,
        }));
    }

    for entry in storage.1 {
        if let Some(pattern) = pattern {
            if !entry.key.contains(pattern) {
                continue;
            }
        }
        total_size_bytes += entry.size_bytes;
        session_storage.push(json!({
            "key": entry.key,
            "value": entry.value,
            "size_bytes": entry.size_bytes,
        }));
    }

    let total_size_kb = total_size_bytes as f64 / 1024.0;

    let result = json!({
        "local_storage": local_storage,
        "session_storage": session_storage,
        "summary": {
            "local_entries": local_storage.len(),
            "session_entries": session_storage.len(),
            "total_size_kb": total_size_kb,
        }
    });

    Ok(ToolEffect::reply_json(&result))
}

fn extract_domain(url: &str) -> String {
    if let Ok(parsed) = url.parse::<url::Url>() {
        parsed.host_str().unwrap_or("").to_string()
    } else {
        String::new()
    }
}
