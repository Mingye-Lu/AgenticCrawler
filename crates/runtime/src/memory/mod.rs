pub mod context;
pub mod episode_builder;
pub mod evidence;
pub mod evidence_aggregator;
pub mod skill_suggestions;
pub use context::{MemoryContext, MemoryContextBudget, MemoryContextLoader, MemoryContextQuery};
pub use episode_builder::{
    build_memory_episode, MemoryEpisodeBuildConfig, MemoryEpisodeBuildInput,
};
pub use evidence::{AccessEvidence, AccessStatus, DomainEvidence, EvidenceStore, TaskEvidence};
pub use evidence_aggregator::{
    aggregate_evidence_from_episodes, EvidenceAggregationConfig, EvidenceAggregationResult,
};
pub use skill_suggestions::{
    suggest_skills_from_evidence, SkillSuggestion, SkillSuggestionConfig, SkillSuggestionKind,
};

use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryEpisode {
    pub id: String,
    pub task_class: Option<String>,
    pub user_goal: String,
    pub route: Vec<String>,
    pub domains: Vec<String>,
    pub tools: Vec<String>,
    pub result: MemoryEpisodeResult,
    pub output_summary: Option<String>,
    pub created_at_epoch_secs: u64,
    pub promote_candidate: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryEpisodeResult {
    Success,
    Partial,
    Failure,
}

#[derive(Debug)]
pub enum MemoryError {
    Io(std::io::Error),
    Json(serde_json::Error),
}

impl Display for MemoryError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "{error}"),
            Self::Json(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for MemoryError {}

impl From<std::io::Error> for MemoryError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<serde_json::Error> for MemoryError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

#[derive(Debug, Clone)]
pub struct EpisodeStore {
    root: PathBuf,
}

impl EpisodeStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    #[must_use]
    pub fn default_for_config_home() -> Self {
        Self::new(crate::config_home_dir())
    }

    #[must_use]
    pub fn episodes_path(&self) -> PathBuf {
        self.root.join("memory").join("episodes.jsonl")
    }

    pub fn append_episode(&self, episode: &MemoryEpisode) -> Result<(), MemoryError> {
        let path = self.episodes_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let mut line = serde_json::to_string(episode)?;
        line.push('\n');

        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        file.write_all(line.as_bytes())?;

        Ok(())
    }

    pub fn load_recent_episodes(&self, limit: usize) -> Result<Vec<MemoryEpisode>, MemoryError> {
        let path = self.episodes_path();
        if !path.exists() {
            return Ok(Vec::new());
        }

        let file = fs::File::open(&path)?;
        let reader = BufReader::new(file);
        let mut all_episodes: Vec<MemoryEpisode> = reader
            .lines()
            .filter_map(|line| line.ok().and_then(|l| serde_json::from_str(&l).ok()))
            .collect();

        let start = all_episodes.len().saturating_sub(limit);
        Ok(all_episodes.split_off(start))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store() -> EpisodeStore {
        let dir = std::env::temp_dir().join(format!(
            "acrawl_memory_test_{}_{}",
            std::process::id(),
            next_test_id()
        ));
        EpisodeStore::new(dir)
    }

    fn next_test_id() -> u64 {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        COUNTER.fetch_add(1, Ordering::Relaxed)
    }

    fn make_episode(id: &str, result: MemoryEpisodeResult, created_at: u64) -> MemoryEpisode {
        MemoryEpisode {
            id: id.to_string(),
            task_class: Some("test".to_string()),
            user_goal: "test goal".to_string(),
            route: vec!["navigate".to_string()],
            domains: vec!["example.com".to_string()],
            tools: vec!["navigate".to_string()],
            result,
            output_summary: Some("summary".to_string()),
            created_at_epoch_secs: created_at,
            promote_candidate: false,
        }
    }

    #[test]
    fn missing_episodes_file_returns_empty() {
        let store = temp_store();
        let result = store.load_recent_episodes(10).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn append_then_load_roundtrip() {
        let store = temp_store();
        let episode = make_episode("ep1", MemoryEpisodeResult::Success, 1000);
        store.append_episode(&episode).unwrap();

        let loaded = store.load_recent_episodes(10).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0], episode);

        let _ = fs::remove_dir_all(&store.root);
    }

    #[test]
    fn load_recent_respects_limit_and_order() {
        let store = temp_store();
        for i in 1..=5 {
            let episode = make_episode(&format!("ep{i}"), MemoryEpisodeResult::Success, i * 100);
            store.append_episode(&episode).unwrap();
        }

        let loaded = store.load_recent_episodes(3).unwrap();
        assert_eq!(loaded.len(), 3);
        assert_eq!(loaded[0].id, "ep3");
        assert_eq!(loaded[1].id, "ep4");
        assert_eq!(loaded[2].id, "ep5");

        let all = store.load_recent_episodes(10).unwrap();
        assert_eq!(all.len(), 5);

        let _ = fs::remove_dir_all(&store.root);
    }

    #[test]
    fn append_creates_memory_directory() {
        let store = temp_store();
        assert!(!store.episodes_path().exists());

        let episode = make_episode("ep_dir", MemoryEpisodeResult::Success, 1000);
        store.append_episode(&episode).unwrap();

        assert!(store.episodes_path().exists());

        let _ = fs::remove_dir_all(&store.root);
    }

    #[test]
    fn result_serialization_is_snake_case() {
        let success = MemoryEpisodeResult::Success;
        let json = serde_json::to_string(&success).unwrap();
        assert_eq!(json, "\"success\"");

        let partial = MemoryEpisodeResult::Partial;
        let json = serde_json::to_string(&partial).unwrap();
        assert_eq!(json, "\"partial\"");

        let failure = MemoryEpisodeResult::Failure;
        let json = serde_json::to_string(&failure).unwrap();
        assert_eq!(json, "\"failure\"");
    }

    #[test]
    fn limit_zero_returns_empty() {
        let store = temp_store();
        store
            .append_episode(&make_episode("ep1", MemoryEpisodeResult::Success, 1000))
            .unwrap();
        let result = store.load_recent_episodes(0).unwrap();
        assert!(result.is_empty());

        let _ = fs::remove_dir_all(&store.root);
    }

    #[test]
    fn malformed_lines_are_skipped() {
        let store = temp_store();
        let path = store.episodes_path();

        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            "not valid json\n{\"id\":\"ep1\",\"task_class\":null,\"user_goal\":\"test\",\"route\":[],\"domains\":[],\"tools\":[],\"result\":\"success\",\"output_summary\":null,\"created_at_epoch_secs\":1000,\"promote_candidate\":false}\n",
        )
        .unwrap();

        let loaded = store.load_recent_episodes(10).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, "ep1");

        let _ = fs::remove_dir_all(&store.root);
    }
}
