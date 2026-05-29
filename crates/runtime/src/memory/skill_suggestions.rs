use super::{DomainEvidence, TaskEvidence};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillSuggestionKind {
    Task,
    Domain,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SkillSuggestion {
    pub kind: SkillSuggestionKind,
    pub key: String,
    pub reason: String,
    pub confidence: f64,
    pub success_count: u32,
    pub failure_count: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct SkillSuggestionConfig {
    pub min_success_count: u32,
    pub min_success_rate: f64,
    pub max_suggestions: usize,
}

impl Default for SkillSuggestionConfig {
    fn default() -> Self {
        Self {
            min_success_count: 3,
            min_success_rate: 0.75,
            max_suggestions: 10,
        }
    }
}

#[must_use]
pub fn suggest_skills_from_evidence(
    tasks: &[TaskEvidence],
    domains: &[DomainEvidence],
    config: SkillSuggestionConfig,
) -> Vec<SkillSuggestion> {
    let mut suggestions: Vec<SkillSuggestion> = Vec::new();

    for t in tasks {
        let total = t.success_count + t.failure_count;
        if total == 0 {
            continue;
        }
        let rate = f64::from(t.success_count) / f64::from(total);
        if t.success_count < config.min_success_count {
            continue;
        }
        if rate < config.min_success_rate {
            continue;
        }
        let reason = if t.successful_routes.is_empty() {
            "task class has repeated successful outcomes"
        } else {
            "task class has repeated successful routes/tools"
        };
        suggestions.push(SkillSuggestion {
            kind: SkillSuggestionKind::Task,
            key: t.task_class.clone(),
            reason: reason.to_string(),
            confidence: rate,
            success_count: t.success_count,
            failure_count: t.failure_count,
        });
    }

    for d in domains {
        let total = d.success_count + d.failure_count;
        if total == 0 {
            continue;
        }
        let rate = f64::from(d.success_count) / f64::from(total);
        if d.success_count < config.min_success_count {
            continue;
        }
        if rate < config.min_success_rate {
            continue;
        }
        suggestions.push(SkillSuggestion {
            kind: SkillSuggestionKind::Domain,
            key: d.domain.clone(),
            reason: "domain has repeated successful crawls".to_string(),
            confidence: rate,
            success_count: d.success_count,
            failure_count: d.failure_count,
        });
    }

    suggestions.sort_by(|a, b| {
        b.confidence
            .partial_cmp(&a.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.success_count.cmp(&a.success_count))
            .then_with(|| a.key.cmp(&b.key))
    });

    suggestions.truncate(config.max_suggestions);
    suggestions
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_task(task_class: &str, success: u32, failure: u32, has_routes: bool) -> TaskEvidence {
        TaskEvidence {
            task_class: task_class.to_string(),
            successful_routes: if has_routes {
                vec![vec!["navigate".to_string()]]
            } else {
                vec![]
            },
            tools: vec!["navigate".to_string()],
            output_fields: vec![],
            success_count: success,
            failure_count: failure,
            last_used_epoch_secs: 1,
        }
    }

    fn make_domain(domain: &str, success: u32, failure: u32) -> DomainEvidence {
        DomainEvidence {
            domain: domain.to_string(),
            task_classes: vec!["scrape".to_string()],
            successful_routes: vec![vec!["navigate".to_string()]],
            field_hints: vec![],
            success_count: success,
            failure_count: failure,
            last_verified_epoch_secs: 1,
        }
    }

    #[test]
    fn no_evidence_returns_empty() {
        let tasks: Vec<TaskEvidence> = vec![];
        let domains: Vec<DomainEvidence> = vec![];
        let result =
            suggest_skills_from_evidence(&tasks, &domains, SkillSuggestionConfig::default());
        assert!(result.is_empty());
    }

    #[test]
    fn below_threshold_is_skipped() {
        let tasks = vec![make_task("scrape", 2, 0, true)];
        let domains: Vec<DomainEvidence> = vec![];
        let result =
            suggest_skills_from_evidence(&tasks, &domains, SkillSuggestionConfig::default());
        assert!(result.is_empty());
    }

    #[test]
    fn failure_heavy_is_filtered_out() {
        let tasks = vec![make_task("scrape", 3, 5, true)];
        let domains: Vec<DomainEvidence> = vec![];
        let result =
            suggest_skills_from_evidence(&tasks, &domains, SkillSuggestionConfig::default());
        assert!(result.is_empty());
    }

    #[test]
    fn successful_task_produces_suggestion() {
        let tasks = vec![make_task("scrape-prices", 5, 1, true)];
        let domains: Vec<DomainEvidence> = vec![];
        let result =
            suggest_skills_from_evidence(&tasks, &domains, SkillSuggestionConfig::default());
        assert_eq!(result.len(), 1);
        assert!(matches!(result[0].kind, SkillSuggestionKind::Task));
        assert_eq!(result[0].key, "scrape-prices");
        assert!(result[0].reason.contains("routes/tools"));
        assert_eq!(result[0].success_count, 5);
        assert_eq!(result[0].failure_count, 1);
        assert!((result[0].confidence - 0.833).abs() < 0.01);
    }

    #[test]
    fn task_without_routes_uses_alternative_reason() {
        let tasks = vec![make_task("check-status", 5, 1, false)];
        let domains: Vec<DomainEvidence> = vec![];
        let result =
            suggest_skills_from_evidence(&tasks, &domains, SkillSuggestionConfig::default());
        assert_eq!(result.len(), 1);
        assert!(result[0].reason.contains("outcomes"));
    }

    #[test]
    fn successful_domain_produces_suggestion() {
        let tasks: Vec<TaskEvidence> = vec![];
        let domains = vec![make_domain("example.com", 10, 2)];
        let result =
            suggest_skills_from_evidence(&tasks, &domains, SkillSuggestionConfig::default());
        assert_eq!(result.len(), 1);
        assert!(matches!(result[0].kind, SkillSuggestionKind::Domain));
        assert_eq!(result[0].key, "example.com");
        assert_eq!(result[0].success_count, 10);
    }

    #[test]
    fn output_sorted_by_confidence_desc() {
        let tasks = vec![
            make_task("task-a", 5, 0, true), // confidence 1.0
            make_task("task-b", 5, 1, true), // confidence ~0.833
            make_task("task-c", 8, 0, true), // confidence 1.0, higher success
        ];
        let domains: Vec<DomainEvidence> = vec![];
        let result =
            suggest_skills_from_evidence(&tasks, &domains, SkillSuggestionConfig::default());
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].key, "task-c"); // confidence 1.0, success 8
        assert_eq!(result[1].key, "task-a"); // confidence 1.0, success 5
        assert_eq!(result[2].key, "task-b"); // confidence ~0.833
    }

    #[test]
    fn max_suggestions_respected() {
        let mut tasks: Vec<TaskEvidence> = Vec::new();
        for i in 1..=15 {
            tasks.push(make_task(
                &format!("task-{i:02}"),
                5_u32.saturating_add(i),
                1,
                true,
            ));
        }
        let domains: Vec<DomainEvidence> = vec![];

        let config = SkillSuggestionConfig {
            max_suggestions: 5,
            ..SkillSuggestionConfig::default()
        };
        let result = suggest_skills_from_evidence(&tasks, &domains, config);
        assert_eq!(result.len(), 5);
    }

    #[test]
    fn exact_threshold_passes() {
        let tasks = vec![make_task("scrape", 3, 1, true)]; // rate = 0.75, exactly at threshold
        let domains: Vec<DomainEvidence> = vec![];
        let result =
            suggest_skills_from_evidence(&tasks, &domains, SkillSuggestionConfig::default());
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn zero_total_is_skipped() {
        let tasks = vec![make_task("empty", 0, 0, true)];
        let domains: Vec<DomainEvidence> = vec![];
        let result =
            suggest_skills_from_evidence(&tasks, &domains, SkillSuggestionConfig::default());
        assert!(result.is_empty());
    }

    #[test]
    fn tie_break_by_key_asc() {
        let tasks = vec![
            make_task("task-z", 5, 0, true),
            make_task("task-a", 5, 0, true),
            make_task("task-m", 5, 0, true),
        ];
        let domains: Vec<DomainEvidence> = vec![];
        let result =
            suggest_skills_from_evidence(&tasks, &domains, SkillSuggestionConfig::default());
        assert_eq!(result.len(), 3);
        // all confidence 1.0, all success 5, sort by key asc
        assert_eq!(result[0].key, "task-a");
        assert_eq!(result[1].key, "task-m");
        assert_eq!(result[2].key, "task-z");
    }
}
