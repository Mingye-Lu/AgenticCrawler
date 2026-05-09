use std::cmp::Ordering;
use std::fs;
use std::io;
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

use crate::config_home_dir;

const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const RELEASES_URL: &str = "https://api.github.com/repos/Mingye-Lu/AgenticCrawler/releases/latest";
const UPDATE_CHECK_CACHE_FILE: &str = ".update-check.json";
const UPDATE_CHECK_TIMEOUT: Duration = Duration::from_secs(2);
const UPDATE_CHECK_TTL_SECS: i64 = 24 * 60 * 60;
const USER_AGENT: &str = "acrawl";

#[derive(Debug, Clone)]
pub struct UpdateInfo {
    pub latest_version: String,
    pub current_version: String,
    pub is_outdated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct UpdateCache {
    latest_version: String,
    checked_at: String,
}

#[derive(Debug, Deserialize)]
struct ReleaseResponse {
    tag_name: String,
}

pub async fn check_for_update() -> Option<UpdateInfo> {
    check_for_update_with_version_and_url(CURRENT_VERSION, RELEASES_URL).await
}

async fn check_for_update_with_version_and_url(
    current_version: &str,
    release_url: &str,
) -> Option<UpdateInfo> {
    let cache_path = cache_file_path();

    if let Some(cache) = read_cache(&cache_path) {
        if is_cache_fresh(&cache) {
            return build_update_info(current_version, &cache.latest_version);
        }
    }

    let latest_version = fetch_latest_version(release_url).await?;
    let cache = UpdateCache {
        latest_version,
        checked_at: current_timestamp_string()?,
    };

    write_cache(&cache_path, &cache).ok()?;
    build_update_info(current_version, &cache.latest_version)
}

fn cache_file_path() -> std::path::PathBuf {
    config_home_dir().join(UPDATE_CHECK_CACHE_FILE)
}

fn read_cache(path: &Path) -> Option<UpdateCache> {
    let content = fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

fn write_cache(path: &Path, cache: &UpdateCache) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let json = serde_json::to_string(cache)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    fs::write(path, json)
}

fn is_cache_fresh(cache: &UpdateCache) -> bool {
    let checked_at = parse_timestamp(&cache.checked_at);
    let now = current_timestamp();

    checked_at.is_some_and(|checked_at| {
        checked_at <= now && now - checked_at < time::Duration::seconds(UPDATE_CHECK_TTL_SECS)
    })
}

async fn fetch_latest_version(release_url: &str) -> Option<String> {
    let client = reqwest::Client::new();
    let release = tokio::time::timeout(UPDATE_CHECK_TIMEOUT, async {
        let response = client
            .get(release_url)
            .timeout(UPDATE_CHECK_TIMEOUT)
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .send()
            .await?;

        response.error_for_status()?.json::<ReleaseResponse>().await
    })
    .await
    .ok()?
    .ok()?;

    Some(strip_version_prefix(&release.tag_name).to_string())
}

fn strip_version_prefix(version: &str) -> &str {
    version.strip_prefix('v').unwrap_or(version)
}

fn build_update_info(current_version: &str, latest_version: &str) -> Option<UpdateInfo> {
    if latest_version.contains('-') {
        return None;
    }

    let is_outdated = compare_versions(current_version, latest_version)? == Ordering::Less;

    Some(UpdateInfo {
        latest_version: latest_version.to_string(),
        current_version: current_version.to_string(),
        is_outdated,
    })
}

fn compare_versions(current_version: &str, latest_version: &str) -> Option<Ordering> {
    let current_parts = parse_version(current_version)?;
    let latest_parts = parse_version(latest_version)?;
    let max_len = current_parts.len().max(latest_parts.len());

    for index in 0..max_len {
        let current = *current_parts.get(index).unwrap_or(&0);
        let latest = *latest_parts.get(index).unwrap_or(&0);

        match current.cmp(&latest) {
            Ordering::Equal => {}
            ordering => return Some(ordering),
        }
    }

    Some(Ordering::Equal)
}

fn parse_version(version: &str) -> Option<Vec<u32>> {
    version
        .split('.')
        .map(|part| part.parse::<u32>().ok())
        .collect()
}

fn current_timestamp() -> OffsetDateTime {
    OffsetDateTime::now_utc()
}

fn current_timestamp_string() -> Option<String> {
    current_timestamp().format(&Rfc3339).ok()
}

fn parse_timestamp(timestamp: &str) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(timestamp, &Rfc3339).ok()
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use time::{format_description::well_known::Rfc3339, OffsetDateTime};

    use super::{
        build_update_info, cache_file_path, check_for_update_with_version_and_url,
        current_timestamp, UpdateCache,
    };

    fn test_env_lock() -> std::sync::MutexGuard<'static, ()> {
        crate::test_env_lock()
    }

    fn setup_temp_dir() -> PathBuf {
        let temp_dir = std::env::temp_dir().join(format!(
            "acrawl_update_check_test_{}_{}",
            std::process::id(),
            OffsetDateTime::now_utc().unix_timestamp_nanos()
        ));
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).expect("Failed to create temp dir");
        temp_dir
    }

    fn cleanup_temp_dir(path: &Path) {
        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn test_version_compare_older() {
        let info = build_update_info("0.2.0", "0.3.0").expect("Expected update info");
        assert!(info.is_outdated);
    }

    #[test]
    fn test_version_compare_equal() {
        let info = build_update_info("1.0.0", "1.0.0").expect("Expected update info");
        assert!(!info.is_outdated);
    }

    #[test]
    fn test_version_compare_major() {
        let info = build_update_info("0.2.0", "0.10.0").expect("Expected update info");
        assert!(info.is_outdated);
    }

    #[test]
    fn test_prerelease_filtered() {
        assert!(build_update_info("0.2.0", "0.3.0-rc1").is_none());
    }

    #[tokio::test]
    async fn test_cache_freshness() {
        let _lock = test_env_lock();
        let temp_dir = setup_temp_dir();
        std::env::set_var("ACRAWL_CONFIG_HOME", &temp_dir);

        let cache = UpdateCache {
            latest_version: "9.9.9".to_string(),
            checked_at: current_timestamp()
                .checked_sub(time::Duration::hours(1))
                .expect("Expected valid timestamp")
                .format(&Rfc3339)
                .expect("Expected RFC3339 timestamp"),
        };
        let cache_json = serde_json::to_string(&cache).expect("Expected cache json");
        std::fs::write(cache_file_path(), cache_json).expect("Expected cache write");

        let update = check_for_update_with_version_and_url("0.1.0", "not a url").await;
        let update = update.expect("Expected cached update info");

        assert_eq!(update.current_version, "0.1.0");
        assert_eq!(update.latest_version, "9.9.9");
        assert!(update.is_outdated);

        cleanup_temp_dir(&temp_dir);
    }

    #[tokio::test]
    async fn test_graceful_failure() {
        let _lock = test_env_lock();
        let temp_dir = setup_temp_dir();
        std::env::set_var("ACRAWL_CONFIG_HOME", &temp_dir);

        let update = check_for_update_with_version_and_url("0.1.0", "not a url").await;

        assert!(update.is_none());

        cleanup_temp_dir(&temp_dir);
    }
}
