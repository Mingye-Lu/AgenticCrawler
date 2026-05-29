use std::fmt::Write;

use super::{EpisodeStore, EvidenceStore, MemoryEpisode, MemoryError};

#[derive(Debug, Clone)]
pub struct MemoryContextQuery {
    pub task_class: Option<String>,
    pub domains: Vec<String>,
    pub include_access: bool,
    pub recent_episode_limit: usize,
}

#[derive(Debug, Clone, Copy)]
pub struct MemoryContextBudget {
    pub max_task_evidence: usize,
    pub max_domain_evidence: usize,
    pub max_access_evidence: usize,
    pub max_recent_episodes: usize,
    pub max_rendered_chars: usize,
}

impl Default for MemoryContextBudget {
    fn default() -> Self {
        Self {
            max_task_evidence: 1,
            max_domain_evidence: 3,
            max_access_evidence: 3,
            max_recent_episodes: 2,
            max_rendered_chars: 4000,
        }
    }
}

#[derive(Debug, Clone)]
pub struct MemoryContext {
    pub task_evidence: Vec<super::TaskEvidence>,
    pub domain_evidence: Vec<super::DomainEvidence>,
    pub access_evidence: Vec<super::AccessEvidence>,
    pub recent_episodes: Vec<MemoryEpisode>,
}

#[derive(Debug, Clone)]
pub struct MemoryContextLoader {
    episode_store: EpisodeStore,
    evidence_store: EvidenceStore,
}

impl MemoryContextLoader {
    #[must_use]
    pub fn new(episode_store: EpisodeStore, evidence_store: EvidenceStore) -> Self {
        Self {
            episode_store,
            evidence_store,
        }
    }

    #[must_use]
    pub fn default_for_config_home() -> Self {
        Self::new(
            EpisodeStore::default_for_config_home(),
            EvidenceStore::default_for_config_home(),
        )
    }

    pub fn load(
        &self,
        query: &MemoryContextQuery,
        budget: MemoryContextBudget,
    ) -> Result<MemoryContext, MemoryError> {
        let mut context = MemoryContext {
            task_evidence: Vec::new(),
            domain_evidence: Vec::new(),
            access_evidence: Vec::new(),
            recent_episodes: Vec::new(),
        };

        if let Some(task_class) = &query.task_class {
            if budget.max_task_evidence > 0 {
                if let Some(evidence) = self.evidence_store.load_task_evidence(task_class)? {
                    context.task_evidence.push(evidence);
                }
            }
        }

        let mut deduped_domains: Vec<&str> = Vec::with_capacity(query.domains.len());
        for domain in &query.domains {
            if deduped_domains.contains(&domain.as_str()) {
                continue;
            }
            deduped_domains.push(domain);
        }

        for domain in &deduped_domains {
            if context.domain_evidence.len() >= budget.max_domain_evidence {
                break;
            }
            if let Some(evidence) = self.evidence_store.load_domain_evidence(domain)? {
                context.domain_evidence.push(evidence);
            }
        }

        if query.include_access {
            for domain in &deduped_domains {
                if context.access_evidence.len() >= budget.max_access_evidence {
                    break;
                }
                if let Some(evidence) = self.evidence_store.load_access_evidence(domain)? {
                    context.access_evidence.push(evidence);
                }
            }
        }

        let episode_limit = query.recent_episode_limit.min(budget.max_recent_episodes);
        if episode_limit > 0 {
            context.recent_episodes = self.episode_store.load_recent_episodes(episode_limit)?;
        }

        Ok(context)
    }

    #[must_use]
    pub fn render_context(&self, context: &MemoryContext, budget: MemoryContextBudget) -> String {
        let has_task = !context.task_evidence.is_empty();
        let has_domain = !context.domain_evidence.is_empty();
        let has_access = !context.access_evidence.is_empty();
        let has_episodes = !context.recent_episodes.is_empty();

        if !has_task && !has_domain && !has_access && !has_episodes {
            return String::new();
        }

        let mut output = String::from("Relevant memory:\n\n");

        if has_task {
            output.push_str("## Task evidence\n\n");
            for task in &context.task_evidence {
                let _ = writeln!(
                    &mut output,
                    "- `{}`: {} success / {} failure | tools: {}",
                    task.task_class,
                    task.success_count,
                    task.failure_count,
                    task.tools.join(", "),
                );
            }
            output.push('\n');
        }

        if has_domain {
            output.push_str("## Domain evidence\n\n");
            for domain in &context.domain_evidence {
                let hints = if domain.field_hints.is_empty() {
                    "none"
                } else {
                    &domain.field_hints.join(", ")
                };
                let _ = writeln!(
                    &mut output,
                    "- `{}`: {} success / {} failure | hints: {}",
                    domain.domain, domain.success_count, domain.failure_count, hints,
                );
            }
            output.push('\n');
        }

        if has_access {
            output.push_str("## Access evidence\n\n");
            for access in &context.access_evidence {
                let _ = writeln!(
                    &mut output,
                    "- `{}`: status={:?} | extension={}",
                    access.domain, access.status, access.extension_mode,
                );
            }
            output.push('\n');
        }

        if has_episodes {
            output.push_str("## Recent episodes\n\n");
            for ep in &context.recent_episodes {
                let goal = truncate_str(&ep.user_goal, 80);
                let tc = ep.task_class.as_deref().unwrap_or("none");
                let _ = writeln!(
                    &mut output,
                    "- `{}` [{}] {:?}: {}",
                    ep.id, tc, ep.result, goal,
                );
            }
            output.push('\n');
        }

        enforce_max_chars(&mut output, budget.max_rendered_chars);

        output
    }
}

fn truncate_str(s: &str, max_chars: usize) -> String {
    if s.chars().count() > max_chars {
        let truncated: String = s.chars().take(max_chars.saturating_sub(3)).collect();
        format!("{truncated}...")
    } else {
        s.to_string()
    }
}

fn enforce_max_chars(output: &mut String, max_chars: usize) {
    if output.chars().count() <= max_chars {
        return;
    }
    if max_chars == 0 {
        output.clear();
        return;
    }

    let note = "\n(truncated)";
    let note_len = note.chars().count();
    if max_chars <= note_len {
        *output = note.chars().take(max_chars).collect();
        return;
    }

    let target = max_chars.saturating_sub(note_len);
    let truncated: String = output.chars().take(target).collect();
    *output = truncated + note;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{AccessEvidence, AccessStatus, DomainEvidence, TaskEvidence};
    use std::fs;

    fn temp_dir() -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "acrawl_context_test_{}_{}",
            std::process::id(),
            next_test_id()
        ))
    }

    fn temp_stores() -> (EpisodeStore, EvidenceStore, std::path::PathBuf) {
        let dir = temp_dir();
        (
            EpisodeStore::new(dir.clone()),
            EvidenceStore::new(dir.clone()),
            dir,
        )
    }

    fn next_test_id() -> u64 {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        COUNTER.fetch_add(1, Ordering::Relaxed)
    }

    fn make_task_evidence(task_class: &str) -> TaskEvidence {
        TaskEvidence {
            task_class: task_class.to_string(),
            successful_routes: vec![vec!["navigate".to_string()]],
            tools: vec!["navigate".to_string(), "extract".to_string()],
            output_fields: vec!["title".to_string()],
            success_count: 5,
            failure_count: 1,
            last_used_epoch_secs: 1_717_000_000,
        }
    }

    fn make_domain_evidence(domain: &str) -> DomainEvidence {
        DomainEvidence {
            domain: domain.to_string(),
            task_classes: vec!["scrape".to_string()],
            successful_routes: vec![vec!["navigate".to_string()]],
            field_hints: vec!["title".to_string(), ".price".to_string()],
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

    fn make_episode(
        episode_store: &EpisodeStore,
        id: &str,
        task_class: Option<&str>,
        created_at: u64,
    ) {
        let episode = crate::memory::MemoryEpisode {
            id: id.to_string(),
            task_class: task_class.map(String::from),
            user_goal: "test goal".to_string(),
            route: vec!["navigate".to_string()],
            domains: vec!["example.com".to_string()],
            tools: vec!["navigate".to_string()],
            result: crate::memory::MemoryEpisodeResult::Success,
            output_summary: Some("summary".to_string()),
            created_at_epoch_secs: created_at,
            promote_candidate: false,
        };
        episode_store.append_episode(&episode).unwrap();
    }

    #[test]
    fn empty_context_renders_empty_string() {
        let (es, evs, root) = temp_stores();
        let loader = MemoryContextLoader::new(es, evs);
        let context = MemoryContext {
            task_evidence: Vec::new(),
            domain_evidence: Vec::new(),
            access_evidence: Vec::new(),
            recent_episodes: Vec::new(),
        };
        let rendered = loader.render_context(&context, MemoryContextBudget::default());
        assert_eq!(rendered, "");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn load_exact_task_evidence_only() {
        let (es, evs, root) = temp_stores();
        evs.save_task_evidence(&make_task_evidence("scrape-prices"))
            .unwrap();
        evs.save_task_evidence(&make_task_evidence("login-check"))
            .unwrap();
        evs.save_domain_evidence(&make_domain_evidence("example.com"))
            .unwrap();

        let loader = MemoryContextLoader::new(es, evs);
        let query = MemoryContextQuery {
            task_class: Some("scrape-prices".to_string()),
            domains: Vec::new(),
            include_access: false,
            recent_episode_limit: 0,
        };
        let context = loader.load(&query, MemoryContextBudget::default()).unwrap();

        assert_eq!(context.task_evidence.len(), 1);
        assert_eq!(context.task_evidence[0].task_class, "scrape-prices");
        assert!(context.domain_evidence.is_empty());
        assert!(context.access_evidence.is_empty());
        assert!(context.recent_episodes.is_empty());

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn load_domain_evidence_preserves_query_order() {
        let (es, evs, root) = temp_stores();
        evs.save_domain_evidence(&make_domain_evidence("c.com"))
            .unwrap();
        evs.save_domain_evidence(&make_domain_evidence("a.com"))
            .unwrap();
        evs.save_domain_evidence(&make_domain_evidence("b.com"))
            .unwrap();

        let loader = MemoryContextLoader::new(es, evs);
        let query = MemoryContextQuery {
            task_class: None,
            domains: vec![
                "c.com".to_string(),
                "a.com".to_string(),
                "b.com".to_string(),
            ],
            include_access: false,
            recent_episode_limit: 0,
        };
        let context = loader.load(&query, MemoryContextBudget::default()).unwrap();

        assert_eq!(context.domain_evidence.len(), 3);
        assert_eq!(context.domain_evidence[0].domain, "c.com");
        assert_eq!(context.domain_evidence[1].domain, "a.com");
        assert_eq!(context.domain_evidence[2].domain, "b.com");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn duplicate_domains_loaded_once() {
        let (es, evs, root) = temp_stores();
        evs.save_domain_evidence(&make_domain_evidence("a.com"))
            .unwrap();
        evs.save_domain_evidence(&make_domain_evidence("b.com"))
            .unwrap();

        let loader = MemoryContextLoader::new(es, evs);
        let query = MemoryContextQuery {
            task_class: None,
            domains: vec![
                "a.com".to_string(),
                "a.com".to_string(),
                "b.com".to_string(),
            ],
            include_access: false,
            recent_episode_limit: 0,
        };
        let context = loader.load(&query, MemoryContextBudget::default()).unwrap();

        assert_eq!(context.domain_evidence.len(), 2);
        assert_eq!(context.domain_evidence[0].domain, "a.com");
        assert_eq!(context.domain_evidence[1].domain, "b.com");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn include_access_false_does_not_load_access() {
        let (es, evs, root) = temp_stores();
        evs.save_access_evidence(&make_access_evidence("example.com"))
            .unwrap();
        evs.save_domain_evidence(&make_domain_evidence("example.com"))
            .unwrap();

        let loader = MemoryContextLoader::new(es, evs);
        let query = MemoryContextQuery {
            task_class: None,
            domains: vec!["example.com".to_string()],
            include_access: false,
            recent_episode_limit: 0,
        };
        let context = loader.load(&query, MemoryContextBudget::default()).unwrap();

        assert!(context.access_evidence.is_empty());
        assert_eq!(context.domain_evidence.len(), 1);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn include_access_true_loads_access() {
        let (es, evs, root) = temp_stores();
        evs.save_access_evidence(&make_access_evidence("example.com"))
            .unwrap();
        evs.save_domain_evidence(&make_domain_evidence("example.com"))
            .unwrap();

        let loader = MemoryContextLoader::new(es, evs);
        let query = MemoryContextQuery {
            task_class: None,
            domains: vec!["example.com".to_string()],
            include_access: true,
            recent_episode_limit: 0,
        };
        let context = loader.load(&query, MemoryContextBudget::default()).unwrap();

        assert_eq!(context.access_evidence.len(), 1);
        assert_eq!(context.access_evidence[0].domain, "example.com");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn recent_episode_limit_respects_min() {
        let (es, evs, root) = temp_stores();
        for i in 1..=5 {
            make_episode(&es, &format!("ep{i}"), Some("test"), i * 100);
        }

        let loader = MemoryContextLoader::new(es, evs);
        let budget = MemoryContextBudget {
            max_recent_episodes: 2,
            ..MemoryContextBudget::default()
        };
        let query = MemoryContextQuery {
            task_class: None,
            domains: Vec::new(),
            include_access: false,
            recent_episode_limit: 10,
        };
        let context = loader.load(&query, budget).unwrap();

        assert_eq!(context.recent_episodes.len(), 2);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn render_context_includes_expected_sections() {
        let (es, evs, root) = temp_stores();
        let loader = MemoryContextLoader::new(es, evs);

        let context = MemoryContext {
            task_evidence: vec![make_task_evidence("scrape-prices")],
            domain_evidence: vec![make_domain_evidence("example.com")],
            access_evidence: vec![make_access_evidence("example.com")],
            recent_episodes: Vec::new(),
        };
        let rendered = loader.render_context(&context, MemoryContextBudget::default());

        assert!(rendered.contains("Relevant memory:"));
        assert!(rendered.contains("## Task evidence"));
        assert!(rendered.contains("## Domain evidence"));
        assert!(rendered.contains("## Access evidence"));
        assert!(rendered.contains("scrape-prices"));
        assert!(rendered.contains("example.com"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn render_context_respects_max_rendered_chars() {
        let (es, evs, root) = temp_stores();
        let loader = MemoryContextLoader::new(es, evs);

        let evidence = TaskEvidence {
            task_class: "x".repeat(200),
            successful_routes: vec![vec!["navigate".to_string()]],
            tools: vec!["navigate".to_string()],
            output_fields: vec![],
            success_count: 5,
            failure_count: 1,
            last_used_epoch_secs: 1_717_000_000,
        };
        let context = MemoryContext {
            task_evidence: vec![evidence],
            domain_evidence: Vec::new(),
            access_evidence: Vec::new(),
            recent_episodes: Vec::new(),
        };
        let budget = MemoryContextBudget {
            max_rendered_chars: 200,
            ..MemoryContextBudget::default()
        };
        let rendered = loader.render_context(&context, budget);

        assert!(rendered.chars().count() <= 200);
        assert!(rendered.contains("(truncated)"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn render_context_no_recent_episodes_section_when_empty() {
        let (es, evs, root) = temp_stores();
        let loader = MemoryContextLoader::new(es, evs);

        let context = MemoryContext {
            task_evidence: vec![make_task_evidence("scrape-prices")],
            domain_evidence: Vec::new(),
            access_evidence: Vec::new(),
            recent_episodes: Vec::new(),
        };
        let rendered = loader.render_context(&context, MemoryContextBudget::default());

        assert!(!rendered.contains("## Recent episodes"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn budget_max_domain_evidence_caps_loading() {
        let (es, evs, root) = temp_stores();
        evs.save_domain_evidence(&make_domain_evidence("a.com"))
            .unwrap();
        evs.save_domain_evidence(&make_domain_evidence("b.com"))
            .unwrap();
        evs.save_domain_evidence(&make_domain_evidence("c.com"))
            .unwrap();

        let loader = MemoryContextLoader::new(es, evs);
        let budget = MemoryContextBudget {
            max_domain_evidence: 2,
            ..MemoryContextBudget::default()
        };
        let query = MemoryContextQuery {
            task_class: None,
            domains: vec![
                "a.com".to_string(),
                "b.com".to_string(),
                "c.com".to_string(),
            ],
            include_access: false,
            recent_episode_limit: 0,
        };
        let context = loader.load(&query, budget).unwrap();

        assert_eq!(context.domain_evidence.len(), 2);
        assert_eq!(context.domain_evidence[0].domain, "a.com");
        assert_eq!(context.domain_evidence[1].domain, "b.com");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn budget_max_access_evidence_caps_loading() {
        let (es, evs, root) = temp_stores();
        evs.save_access_evidence(&make_access_evidence("a.com"))
            .unwrap();
        evs.save_access_evidence(&make_access_evidence("b.com"))
            .unwrap();
        evs.save_access_evidence(&make_access_evidence("c.com"))
            .unwrap();

        let loader = MemoryContextLoader::new(es, evs);
        let budget = MemoryContextBudget {
            max_access_evidence: 2,
            ..MemoryContextBudget::default()
        };
        let query = MemoryContextQuery {
            task_class: None,
            domains: vec![
                "a.com".to_string(),
                "b.com".to_string(),
                "c.com".to_string(),
            ],
            include_access: true,
            recent_episode_limit: 0,
        };
        let context = loader.load(&query, budget).unwrap();

        assert_eq!(context.access_evidence.len(), 2);
        assert_eq!(context.access_evidence[0].domain, "a.com");
        assert_eq!(context.access_evidence[1].domain, "b.com");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn access_loading_is_not_capped_by_domain_budget() {
        let (es, evs, root) = temp_stores();
        evs.save_access_evidence(&make_access_evidence("a.com"))
            .unwrap();
        evs.save_access_evidence(&make_access_evidence("b.com"))
            .unwrap();

        let loader = MemoryContextLoader::new(es, evs);
        let budget = MemoryContextBudget {
            max_domain_evidence: 0,
            max_access_evidence: 2,
            ..MemoryContextBudget::default()
        };
        let query = MemoryContextQuery {
            task_class: None,
            domains: vec!["a.com".to_string(), "b.com".to_string()],
            include_access: true,
            recent_episode_limit: 0,
        };
        let context = loader.load(&query, budget).unwrap();

        assert!(context.domain_evidence.is_empty());
        assert_eq!(context.access_evidence.len(), 2);
        assert_eq!(context.access_evidence[0].domain, "a.com");
        assert_eq!(context.access_evidence[1].domain, "b.com");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn render_context_respects_tiny_max_rendered_chars() {
        let (es, evs, root) = temp_stores();
        let loader = MemoryContextLoader::new(es, evs);
        let context = MemoryContext {
            task_evidence: vec![make_task_evidence("scrape-prices")],
            domain_evidence: Vec::new(),
            access_evidence: Vec::new(),
            recent_episodes: Vec::new(),
        };

        let rendered = loader.render_context(
            &context,
            MemoryContextBudget {
                max_rendered_chars: 5,
                ..MemoryContextBudget::default()
            },
        );
        assert!(rendered.chars().count() <= 5);

        let empty = loader.render_context(
            &context,
            MemoryContextBudget {
                max_rendered_chars: 0,
                ..MemoryContextBudget::default()
            },
        );
        assert!(empty.is_empty());

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_task_class_yields_none() {
        let (es, evs, root) = temp_stores();
        let loader = MemoryContextLoader::new(es, evs);
        let query = MemoryContextQuery {
            task_class: Some("nonexistent".to_string()),
            domains: Vec::new(),
            include_access: false,
            recent_episode_limit: 0,
        };
        let context = loader.load(&query, MemoryContextBudget::default()).unwrap();

        assert!(context.task_evidence.is_empty());

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn budget_defaults_match_spec() {
        let budget = MemoryContextBudget::default();
        assert_eq!(budget.max_task_evidence, 1);
        assert_eq!(budget.max_domain_evidence, 3);
        assert_eq!(budget.max_access_evidence, 3);
        assert_eq!(budget.max_recent_episodes, 2);
        assert_eq!(budget.max_rendered_chars, 4000);
    }

    #[test]
    fn default_for_config_home_does_not_panic() {
        let loader = MemoryContextLoader::default_for_config_home();
        let query = MemoryContextQuery {
            task_class: None,
            domains: Vec::new(),
            include_access: false,
            recent_episode_limit: 0,
        };
        let result = loader.load(&query, MemoryContextBudget::default());
        assert!(result.is_ok());
    }
}
