use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use runtime::{Session, SessionError};

#[derive(Debug, Clone)]
pub struct SessionHandle {
    pub id: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct ManagedSessionSummary {
    pub id: String,
    pub modified_epoch_secs: u64,
    pub message_count: usize,
    pub title: Option<String>,
}

#[must_use]
pub fn sessions_dir() -> PathBuf {
    runtime::config_home_dir().join("sessions")
}

#[must_use]
pub fn create_managed_session_handle() -> SessionHandle {
    let id = generate_session_id();
    let path = sessions_dir().join(format!("{id}.json"));
    SessionHandle { id, path }
}

fn generate_session_id() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default();
    format!("session-{millis}")
}

pub fn list_managed_sessions() -> Result<Vec<ManagedSessionSummary>, Box<dyn std::error::Error>> {
    let mut sessions = Vec::new();
    let dir = sessions_dir();
    let read_dir = match fs::read_dir(&dir) {
        Ok(rd) => rd,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(sessions),
        Err(err) => return Err(err.into()),
    };
    for entry in read_dir {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let metadata = entry.metadata()?;
        let modified_epoch_secs = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_secs())
            .unwrap_or_default();
        let loaded = Session::load_from_path(&path).ok();
        let message_count = loaded
            .as_ref()
            .map(|s| s.messages.len())
            .unwrap_or_default();
        let title = loaded.and_then(|s| s.title);
        let id = path
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("unknown")
            .to_string();
        sessions.push(ManagedSessionSummary {
            id,
            modified_epoch_secs,
            message_count,
            title,
        });
    }
    sessions.sort_by_key(|s| std::cmp::Reverse(s.modified_epoch_secs));
    Ok(sessions)
}

pub fn delete_session(path: &Path) -> std::io::Result<()> {
    let _ = fs::remove_file(path.with_extension("tmp"));
    fs::remove_file(path)
}

pub fn rename_session(path: &Path, new_title: &str) -> Result<(), SessionError> {
    let mut session = Session::load_from_path(path)?;
    let trimmed = new_title.trim();
    session.title = if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    };
    session.save_to_path(path)
}

#[cfg(test)]
mod tests {
    use super::{delete_session, rename_session};
    use runtime::Session;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_session_path() -> std::path::PathBuf {
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("acrawl-session-mgr-test-{millis}-{n}.json"))
    }

    #[test]
    fn delete_session_removes_file_and_tmp_sibling() {
        let path = temp_session_path();
        Session::new()
            .save_to_path(&path)
            .expect("write session file");
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, b"stale").expect("write stale tmp");

        delete_session(&path).expect("delete");

        assert!(!path.exists(), "session file should be gone");
        assert!(!tmp.exists(), "stale .tmp sibling should be removed");
    }

    #[test]
    fn delete_session_succeeds_without_tmp_sibling() {
        let path = temp_session_path();
        Session::new()
            .save_to_path(&path)
            .expect("write session file");

        delete_session(&path).expect("delete");

        assert!(!path.exists());
    }

    #[test]
    fn rename_session_sets_title_round_trip() {
        let path = temp_session_path();
        Session::new()
            .save_to_path(&path)
            .expect("write session file");

        rename_session(&path, "  Hello World  ").expect("rename");

        let loaded = Session::load_from_path(&path).expect("reload");
        assert_eq!(loaded.title.as_deref(), Some("Hello World"));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn rename_session_with_empty_string_clears_title() {
        let path = temp_session_path();
        let mut session = Session::new();
        session.title = Some("Old Title".to_string());
        session.save_to_path(&path).expect("write");

        rename_session(&path, "   ").expect("rename");

        let loaded = Session::load_from_path(&path).expect("reload");
        assert!(loaded.title.is_none(), "empty title should clear field");

        let _ = std::fs::remove_file(&path);
    }
}
