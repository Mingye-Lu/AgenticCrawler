use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

use super::MemoryError;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessStatus {
    Unknown,
    Registered,
    LoggedInObserved,
    RequiresLogin,
    NotAvailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskEvidence {
    pub task_class: String,
    pub successful_routes: Vec<Vec<String>>,
    pub tools: Vec<String>,
    pub output_fields: Vec<String>,
    pub success_count: u32,
    pub failure_count: u32,
    pub last_used_epoch_secs: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DomainEvidence {
    pub domain: String,
    pub task_classes: Vec<String>,
    pub successful_routes: Vec<Vec<String>>,
    pub field_hints: Vec<String>,
    pub success_count: u32,
    pub failure_count: u32,
    pub last_verified_epoch_secs: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessEvidence {
    pub domain: String,
    pub status: AccessStatus,
    pub extension_mode: bool,
    pub last_confirmed_epoch_secs: u64,
    pub notes: Option<String>,
}

fn sanitize_filename(raw: &str) -> String {
    if raw.is_empty() {
        return "unknown".to_string();
    }
    raw.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[derive(Debug, Clone)]
pub struct EvidenceStore {
    root: PathBuf,
}

impl EvidenceStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    #[must_use]
    pub fn default_for_config_home() -> Self {
        Self::new(crate::config_home_dir())
    }

    #[must_use]
    pub fn evidence_dir(&self) -> PathBuf {
        self.root.join("memory").join("evidence")
    }

    fn tasks_dir(&self) -> PathBuf {
        self.evidence_dir().join("tasks")
    }

    fn domains_dir(&self) -> PathBuf {
        self.evidence_dir().join("domains")
    }

    fn access_dir(&self) -> PathBuf {
        self.evidence_dir().join("access")
    }

    pub fn save_task_evidence(&self, evidence: &TaskEvidence) -> Result<(), MemoryError> {
        let dir = self.tasks_dir();
        fs::create_dir_all(&dir)?;
        let filename = sanitize_filename(&evidence.task_class);
        let path = dir.join(format!("{filename}.json"));
        let json = serde_json::to_string_pretty(evidence)?;
        fs::write(&path, json)?;
        Ok(())
    }

    pub fn load_task_evidence(
        &self,
        task_class: &str,
    ) -> Result<Option<TaskEvidence>, MemoryError> {
        let filename = sanitize_filename(task_class);
        let path = self.tasks_dir().join(format!("{filename}.json"));
        if !path.exists() {
            return Ok(None);
        }
        let data = fs::read_to_string(&path)?;
        let evidence: TaskEvidence = serde_json::from_str(&data)?;
        Ok(Some(evidence))
    }

    pub fn save_domain_evidence(&self, evidence: &DomainEvidence) -> Result<(), MemoryError> {
        let dir = self.domains_dir();
        fs::create_dir_all(&dir)?;
        let filename = sanitize_filename(&evidence.domain);
        let path = dir.join(format!("{filename}.json"));
        let json = serde_json::to_string_pretty(evidence)?;
        fs::write(&path, json)?;
        Ok(())
    }

    pub fn load_domain_evidence(
        &self,
        domain: &str,
    ) -> Result<Option<DomainEvidence>, MemoryError> {
        let filename = sanitize_filename(domain);
        let path = self.domains_dir().join(format!("{filename}.json"));
        if !path.exists() {
            return Ok(None);
        }
        let data = fs::read_to_string(&path)?;
        let evidence: DomainEvidence = serde_json::from_str(&data)?;
        Ok(Some(evidence))
    }

    pub fn save_access_evidence(&self, evidence: &AccessEvidence) -> Result<(), MemoryError> {
        let dir = self.access_dir();
        fs::create_dir_all(&dir)?;
        let filename = sanitize_filename(&evidence.domain);
        let path = dir.join(format!("{filename}.json"));
        let json = serde_json::to_string_pretty(evidence)?;
        fs::write(&path, json)?;
        Ok(())
    }

    pub fn load_access_evidence(
        &self,
        domain: &str,
    ) -> Result<Option<AccessEvidence>, MemoryError> {
        let filename = sanitize_filename(domain);
        let path = self.access_dir().join(format!("{filename}.json"));
        if !path.exists() {
            return Ok(None);
        }
        let data = fs::read_to_string(&path)?;
        let evidence: AccessEvidence = serde_json::from_str(&data)?;
        Ok(Some(evidence))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store() -> EvidenceStore {
        let dir = std::env::temp_dir().join(format!(
            "acrawl_evidence_test_{}_{}",
            std::process::id(),
            next_test_id()
        ));
        EvidenceStore::new(&dir)
    }

    fn next_test_id() -> u64 {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        COUNTER.fetch_add(1, Ordering::Relaxed)
    }

    fn remove_temp(store: &EvidenceStore) {
        let _ = fs::remove_dir_all(&store.root);
    }

    fn make_task_evidence(task_class: &str) -> TaskEvidence {
        TaskEvidence {
            task_class: task_class.to_string(),
            successful_routes: vec![vec!["navigate".to_string(), "extract".to_string()]],
            tools: vec!["navigate".to_string(), "extract".to_string()],
            output_fields: vec!["title".to_string(), "price".to_string()],
            success_count: 5,
            failure_count: 1,
            last_used_epoch_secs: 1_717_000_000,
        }
    }

    fn make_domain_evidence(domain: &str) -> DomainEvidence {
        DomainEvidence {
            domain: domain.to_string(),
            task_classes: vec!["scrape".to_string(), "form-fill".to_string()],
            successful_routes: vec![vec!["navigate".to_string(), "click".to_string()]],
            field_hints: vec!["title h1".to_string(), ".price".to_string()],
            success_count: 10,
            failure_count: 2,
            last_verified_epoch_secs: 1_717_000_000,
        }
    }

    fn make_access_evidence(domain: &str) -> AccessEvidence {
        AccessEvidence {
            domain: domain.to_string(),
            status: AccessStatus::LoggedInObserved,
            extension_mode: false,
            last_confirmed_epoch_secs: 1_717_000_000,
            notes: Some("test note".to_string()),
        }
    }

    #[test]
    fn task_evidence_save_load_roundtrip() {
        let store = temp_store();
        let evidence = make_task_evidence("scrape-prices");
        store.save_task_evidence(&evidence).unwrap();
        let loaded = store
            .load_task_evidence("scrape-prices")
            .unwrap()
            .expect("should load");
        assert_eq!(loaded, evidence);
        remove_temp(&store);
    }

    #[test]
    fn domain_evidence_save_load_roundtrip() {
        let store = temp_store();
        let evidence = make_domain_evidence("example.com");
        store.save_domain_evidence(&evidence).unwrap();
        let loaded = store
            .load_domain_evidence("example.com")
            .unwrap()
            .expect("should load");
        assert_eq!(loaded, evidence);
        remove_temp(&store);
    }

    #[test]
    fn access_evidence_save_load_roundtrip() {
        let store = temp_store();
        let evidence = make_access_evidence("shop.example.com");
        store.save_access_evidence(&evidence).unwrap();
        let loaded = store
            .load_access_evidence("shop.example.com")
            .unwrap()
            .expect("should load");
        assert_eq!(loaded, evidence);
        remove_temp(&store);
    }

    #[test]
    fn missing_evidence_returns_none() {
        let store = temp_store();
        assert!(store.load_task_evidence("nonexistent").unwrap().is_none());
        assert!(store
            .load_domain_evidence("nonexistent.com")
            .unwrap()
            .is_none());
        assert!(store
            .load_access_evidence("nonexistent.com")
            .unwrap()
            .is_none());
        remove_temp(&store);
    }

    #[test]
    fn filename_sanitization_handles_unsafe_chars() {
        assert_eq!(sanitize_filename("hello world"), "hello_world");
        assert_eq!(sanitize_filename("a/b\\c:d*e?f"), "a_b_c_d_e_f");
        assert_eq!(sanitize_filename("keep.ascii-123"), "keep.ascii-123");
        assert_eq!(sanitize_filename(""), "unknown");
        assert_eq!(sanitize_filename("中文"), "__");
    }

    #[test]
    fn access_status_serializes_as_snake_case() {
        let cases = vec![
            (AccessStatus::Unknown, "\"unknown\""),
            (AccessStatus::Registered, "\"registered\""),
            (AccessStatus::LoggedInObserved, "\"logged_in_observed\""),
            (AccessStatus::RequiresLogin, "\"requires_login\""),
            (AccessStatus::NotAvailable, "\"not_available\""),
        ];
        for (status, expected) in cases {
            let json = serde_json::to_string(&status).unwrap();
            assert_eq!(
                json, expected,
                "expected {expected} for {status:?}, got {json}"
            );
        }
    }

    #[test]
    fn save_creates_directories_automatically() {
        let store = temp_store();
        assert!(!store.tasks_dir().exists());
        assert!(!store.domains_dir().exists());
        assert!(!store.access_dir().exists());

        store
            .save_task_evidence(&make_task_evidence("auto-dir"))
            .unwrap();
        store
            .save_domain_evidence(&make_domain_evidence("auto-dir.example.com"))
            .unwrap();
        store
            .save_access_evidence(&make_access_evidence("auto-dir.example.com"))
            .unwrap();

        assert!(store.tasks_dir().exists());
        assert!(store.domains_dir().exists());
        assert!(store.access_dir().exists());

        remove_temp(&store);
    }

    #[test]
    fn load_with_sanitized_key_matches_save() {
        let store = temp_store();
        let evidence = make_task_evidence("task with spaces");
        store.save_task_evidence(&evidence).unwrap();
        let loaded = store
            .load_task_evidence("task with spaces")
            .unwrap()
            .expect("should load");
        assert_eq!(loaded, evidence);
        remove_temp(&store);
    }

    #[test]
    fn pretty_json_produces_readable_output() {
        let store = temp_store();
        let evidence = make_task_evidence("pretty-test");
        store.save_task_evidence(&evidence).unwrap();
        let filename = sanitize_filename("pretty-test");
        let path = store.tasks_dir().join(format!("{filename}.json"));
        let raw = fs::read_to_string(&path).unwrap();
        assert!(raw.contains('\n'), "pretty JSON should have newlines");
        assert!(raw.contains("  "), "pretty JSON should have indentation");
        remove_temp(&store);
    }

    #[test]
    fn empty_domain_uses_unknown_filename() {
        let store = temp_store();
        let mut evidence = make_domain_evidence("");
        evidence.domain = String::new();
        store.save_domain_evidence(&evidence).unwrap();
        let path = store.domains_dir().join("unknown.json");
        assert!(
            path.exists(),
            "empty domain should be saved as unknown.json"
        );
        let loaded = store
            .load_domain_evidence("")
            .unwrap()
            .expect("should load");
        assert_eq!(loaded, evidence);
        remove_temp(&store);
    }
}
