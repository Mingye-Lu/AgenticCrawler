use std::collections::HashSet;

use super::{DomainEvidence, MemoryEpisode, MemoryEpisodeResult, TaskEvidence};

#[derive(Debug, Clone, Copy)]
pub struct EvidenceAggregationConfig {
    pub max_routes_per_task: usize,
    pub max_routes_per_domain: usize,
    pub max_tools_per_task: usize,
    pub max_field_hints_per_domain: usize,
}

impl Default for EvidenceAggregationConfig {
    fn default() -> Self {
        Self {
            max_routes_per_task: 5,
            max_routes_per_domain: 5,
            max_tools_per_task: 16,
            max_field_hints_per_domain: 16,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvidenceAggregationResult {
    pub task_evidence: Vec<TaskEvidence>,
    pub domain_evidence: Vec<DomainEvidence>,
}

#[must_use]
pub fn aggregate_evidence_from_episodes(
    episodes: &[MemoryEpisode],
    config: EvidenceAggregationConfig,
) -> EvidenceAggregationResult {
    let candidates: Vec<&MemoryEpisode> =
        episodes.iter().filter(|ep| ep.promote_candidate).collect();

    let task_evidence = aggregate_task_evidence(&candidates, &config);
    let domain_evidence = aggregate_domain_evidence(&candidates, &config);

    EvidenceAggregationResult {
        task_evidence,
        domain_evidence,
    }
}

/// Filters episodes with `task_class = Some(...)` and groups by `task_class`.
fn episodes_by_task_class<'a>(
    episodes: &[&'a MemoryEpisode],
) -> Vec<(&'a str, Vec<&'a MemoryEpisode>)> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut groups: Vec<(&str, Vec<&MemoryEpisode>)> = Vec::new();

    for ep in episodes {
        if let Some(ref tc) = ep.task_class {
            if seen.insert(tc.clone()) {
                groups.push((tc, Vec::new()));
            }
        }
    }

    for ep in episodes {
        if let Some(ref tc) = ep.task_class {
            if let Some((_, group)) = groups.iter_mut().find(|(key, _)| *key == tc.as_str()) {
                group.push(ep);
            }
        }
    }

    groups.sort_by(|a, b| a.0.cmp(b.0));
    groups
}

/// Groups episodes by domain.
fn episodes_by_domain<'a>(
    episodes: &[&'a MemoryEpisode],
) -> Vec<(&'a str, Vec<&'a MemoryEpisode>)> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut groups: Vec<(&str, Vec<&MemoryEpisode>)> = Vec::new();

    for ep in episodes {
        for domain in &ep.domains {
            if seen.insert(domain.clone()) {
                groups.push((domain, Vec::new()));
            }
        }
    }

    for ep in episodes {
        let mut episode_seen_domains: HashSet<&str> = HashSet::new();
        for domain in &ep.domains {
            if !episode_seen_domains.insert(domain.as_str()) {
                continue;
            }
            if let Some((_, group)) = groups.iter_mut().find(|(key, _)| *key == domain.as_str()) {
                group.push(ep);
            }
        }
    }

    groups.sort_by(|a, b| a.0.cmp(b.0));
    groups
}

/// Preserves first-seen order dedup.
fn dedup_vec_of_vec(items: &[Vec<String>], max: usize) -> Vec<Vec<String>> {
    let mut seen: HashSet<Vec<String>> = HashSet::new();
    let mut result: Vec<Vec<String>> = Vec::new();
    for item in items {
        if result.len() >= max {
            break;
        }
        if seen.insert(item.clone()) {
            result.push(item.clone());
        }
    }
    result
}

/// Preserves first-seen order dedup for flat strings.
fn dedup_strings(items: &[String], max: usize) -> Vec<String> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut result: Vec<String> = Vec::new();
    for item in items {
        if result.len() >= max {
            break;
        }
        if seen.insert(item.clone()) {
            result.push(item.clone());
        }
    }
    result
}

fn aggregate_task_evidence(
    episodes: &[&MemoryEpisode],
    config: &EvidenceAggregationConfig,
) -> Vec<TaskEvidence> {
    let groups = episodes_by_task_class(episodes);

    groups
        .into_iter()
        .map(|(task_class, eps)| {
            let mut successful_routes: Vec<Vec<String>> = Vec::new();
            let mut tools: Vec<String> = Vec::new();
            let mut success_count: u32 = 0;
            let mut failure_count: u32 = 0;
            let mut last_used_epoch_secs: u64 = 0;

            for ep in &eps {
                match ep.result {
                    MemoryEpisodeResult::Success => {
                        successful_routes.push(ep.route.clone());
                        success_count += 1;
                    }
                    MemoryEpisodeResult::Failure => {
                        failure_count += 1;
                    }
                    MemoryEpisodeResult::Partial => {}
                }

                for tool in &ep.tools {
                    tools.push(tool.clone());
                }

                if ep.created_at_epoch_secs > last_used_epoch_secs {
                    last_used_epoch_secs = ep.created_at_epoch_secs;
                }
            }

            TaskEvidence {
                task_class: task_class.to_string(),
                successful_routes: dedup_vec_of_vec(&successful_routes, config.max_routes_per_task),
                tools: dedup_strings(&tools, config.max_tools_per_task),
                output_fields: Vec::new(),
                success_count,
                failure_count,
                last_used_epoch_secs,
            }
        })
        .collect()
}

fn aggregate_domain_evidence(
    episodes: &[&MemoryEpisode],
    config: &EvidenceAggregationConfig,
) -> Vec<DomainEvidence> {
    let groups = episodes_by_domain(episodes);

    groups
        .into_iter()
        .map(|(domain, eps)| {
            let mut task_classes: Vec<String> = Vec::new();
            let mut successful_routes: Vec<Vec<String>> = Vec::new();
            let mut success_count: u32 = 0;
            let mut failure_count: u32 = 0;
            let mut last_verified_epoch_secs: u64 = 0;

            for ep in &eps {
                if let Some(ref tc) = ep.task_class {
                    task_classes.push(tc.clone());
                }

                match ep.result {
                    MemoryEpisodeResult::Success => {
                        successful_routes.push(ep.route.clone());
                        success_count += 1;
                    }
                    MemoryEpisodeResult::Failure => {
                        failure_count += 1;
                    }
                    MemoryEpisodeResult::Partial => {}
                }

                if ep.created_at_epoch_secs > last_verified_epoch_secs {
                    last_verified_epoch_secs = ep.created_at_epoch_secs;
                }
            }

            DomainEvidence {
                domain: domain.to_string(),
                task_classes: dedup_strings(&task_classes, usize::MAX),
                successful_routes: dedup_vec_of_vec(
                    &successful_routes,
                    config.max_routes_per_domain,
                ),
                field_hints: Vec::new(),
                success_count,
                failure_count,
                last_verified_epoch_secs,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(clippy::too_many_arguments)]
    fn make_episode(
        id: &str,
        task_class: Option<&str>,
        route: &[&str],
        domains: &[&str],
        tools: &[&str],
        result: MemoryEpisodeResult,
        created_at: u64,
        promote: bool,
    ) -> MemoryEpisode {
        MemoryEpisode {
            id: id.to_string(),
            task_class: task_class.map(std::string::ToString::to_string),
            user_goal: format!("goal for {id}"),
            route: route.iter().map(std::string::ToString::to_string).collect(),
            domains: domains
                .iter()
                .map(std::string::ToString::to_string)
                .collect(),
            tools: tools.iter().map(std::string::ToString::to_string).collect(),
            result,
            output_summary: Some(format!("summary {id}")),
            created_at_epoch_secs: created_at,
            promote_candidate: promote,
        }
    }

    #[test]
    fn filters_out_promote_candidate_false() {
        let episodes = vec![
            make_episode(
                "e1",
                Some("scrape"),
                &["navigate"],
                &["example.com"],
                &["navigate"],
                MemoryEpisodeResult::Success,
                1000,
                false,
            ),
            make_episode(
                "e2",
                Some("scrape"),
                &["navigate", "extract"],
                &["example.com"],
                &["navigate", "extract"],
                MemoryEpisodeResult::Success,
                2000,
                true,
            ),
        ];

        let result =
            aggregate_evidence_from_episodes(&episodes, EvidenceAggregationConfig::default());

        assert_eq!(result.task_evidence.len(), 1);
        assert_eq!(result.task_evidence[0].task_class, "scrape");
        assert_eq!(result.domain_evidence.len(), 1);
        assert_eq!(result.domain_evidence[0].domain, "example.com");
    }

    #[test]
    fn aggregates_task_evidence_by_task_class() {
        let episodes = vec![
            make_episode(
                "e1",
                Some("scrape"),
                &["navigate"],
                &["example.com"],
                &["navigate"],
                MemoryEpisodeResult::Success,
                1000,
                true,
            ),
            make_episode(
                "e2",
                Some("scrape"),
                &["navigate", "click"],
                &["other.com"],
                &["navigate", "click"],
                MemoryEpisodeResult::Success,
                2000,
                true,
            ),
            make_episode(
                "e3",
                Some("search"),
                &["navigate", "type", "submit"],
                &["google.com"],
                &["navigate", "type", "submit"],
                MemoryEpisodeResult::Success,
                1500,
                true,
            ),
        ];

        let result =
            aggregate_evidence_from_episodes(&episodes, EvidenceAggregationConfig::default());

        assert_eq!(result.task_evidence.len(), 2);
        let scrape = &result.task_evidence[0];
        assert_eq!(scrape.task_class, "scrape");
        let search = &result.task_evidence[1];
        assert_eq!(search.task_class, "search");

        assert_eq!(scrape.success_count, 2);
        assert_eq!(search.success_count, 1);
    }

    #[test]
    fn ignores_task_evidence_for_task_class_none() {
        let episodes = vec![
            make_episode(
                "e1",
                Some("scrape"),
                &["navigate"],
                &["example.com"],
                &["navigate"],
                MemoryEpisodeResult::Success,
                1000,
                true,
            ),
            make_episode(
                "e2",
                None,
                &["navigate"],
                &["unknown.com"],
                &["navigate"],
                MemoryEpisodeResult::Success,
                2000,
                true,
            ),
        ];

        let result =
            aggregate_evidence_from_episodes(&episodes, EvidenceAggregationConfig::default());

        assert_eq!(result.task_evidence.len(), 1);
        assert_eq!(result.task_evidence[0].task_class, "scrape");
    }

    #[test]
    fn aggregates_domain_evidence_by_domain() {
        let episodes = vec![
            make_episode(
                "e1",
                Some("scrape"),
                &["navigate"],
                &["example.com"],
                &["navigate"],
                MemoryEpisodeResult::Success,
                1000,
                true,
            ),
            make_episode(
                "e2",
                Some("search"),
                &["navigate", "type"],
                &["google.com"],
                &["navigate", "type"],
                MemoryEpisodeResult::Success,
                2000,
                true,
            ),
            make_episode(
                "e3",
                Some("scrape"),
                &["navigate", "click"],
                &["example.com"],
                &["navigate", "click"],
                MemoryEpisodeResult::Success,
                3000,
                true,
            ),
        ];

        let result =
            aggregate_evidence_from_episodes(&episodes, EvidenceAggregationConfig::default());

        assert_eq!(result.domain_evidence.len(), 2);
        let ex = &result.domain_evidence[0];
        assert_eq!(ex.domain, "example.com");
        let go = &result.domain_evidence[1];
        assert_eq!(go.domain, "google.com");

        assert_eq!(ex.success_count, 2);
        assert_eq!(go.success_count, 1);
    }

    #[test]
    fn duplicate_domains_within_episode_count_once() {
        let episodes = vec![make_episode(
            "e1",
            Some("scrape"),
            &["navigate"],
            &["example.com", "example.com"],
            &["navigate"],
            MemoryEpisodeResult::Success,
            1000,
            true,
        )];

        let result =
            aggregate_evidence_from_episodes(&episodes, EvidenceAggregationConfig::default());

        assert_eq!(result.domain_evidence.len(), 1);
        assert_eq!(result.domain_evidence[0].success_count, 1);
    }

    #[test]
    fn counts_success_and_failure_ignores_partial() {
        let episodes = vec![
            make_episode(
                "e1",
                Some("scrape"),
                &["navigate"],
                &["example.com"],
                &["navigate"],
                MemoryEpisodeResult::Success,
                1000,
                true,
            ),
            make_episode(
                "e2",
                Some("scrape"),
                &["navigate"],
                &["example.com"],
                &["navigate"],
                MemoryEpisodeResult::Failure,
                2000,
                true,
            ),
            make_episode(
                "e3",
                Some("scrape"),
                &["navigate"],
                &["example.com"],
                &["navigate"],
                MemoryEpisodeResult::Partial,
                3000,
                true,
            ),
        ];

        let result =
            aggregate_evidence_from_episodes(&episodes, EvidenceAggregationConfig::default());

        let task = &result.task_evidence[0];
        assert_eq!(task.success_count, 1);
        assert_eq!(task.failure_count, 1);

        let domain = &result.domain_evidence[0];
        assert_eq!(domain.success_count, 1);
        assert_eq!(domain.failure_count, 1);
    }

    #[test]
    fn dedupes_preserving_first_seen_order() {
        let episodes = vec![
            make_episode(
                "e1",
                Some("scrape"),
                &["navigate", "extract"],
                &["example.com"],
                &["a", "b"],
                MemoryEpisodeResult::Success,
                1000,
                true,
            ),
            make_episode(
                "e2",
                Some("scrape"),
                &["navigate", "extract"],
                &["example.com"],
                &["b", "a"],
                MemoryEpisodeResult::Success,
                2000,
                true,
            ),
            make_episode(
                "e3",
                Some("scrape"),
                &["navigate", "click"],
                &["example.com"],
                &["c"],
                MemoryEpisodeResult::Success,
                3000,
                true,
            ),
        ];

        let result =
            aggregate_evidence_from_episodes(&episodes, EvidenceAggregationConfig::default());

        let task = &result.task_evidence[0];
        assert_eq!(task.successful_routes.len(), 2);
        assert_eq!(task.successful_routes[0], vec!["navigate", "extract"]);
        assert_eq!(task.successful_routes[1], vec!["navigate", "click"]);
        assert_eq!(task.tools[0], "a");

        let domain = &result.domain_evidence[0];
        assert_eq!(domain.task_classes.len(), 1);
        assert_eq!(domain.task_classes[0], "scrape");
        assert_eq!(domain.successful_routes.len(), 2);
    }

    #[test]
    fn respects_max_limits() {
        let mut episodes = Vec::new();
        for i in 0_u64..10 {
            episodes.push(make_episode(
                &format!("e{i}"),
                Some("scrape"),
                &[&format!("step{i}")],
                &["example.com"],
                &[&format!("tool{i}")],
                MemoryEpisodeResult::Success,
                i * 100,
                true,
            ));
        }

        let config = EvidenceAggregationConfig {
            max_routes_per_task: 3,
            max_routes_per_domain: 4,
            max_tools_per_task: 5,
            max_field_hints_per_domain: 16,
        };

        let result = aggregate_evidence_from_episodes(&episodes, config);

        let task = &result.task_evidence[0];
        assert_eq!(task.successful_routes.len(), 3);
        assert_eq!(task.tools.len(), 5);

        let domain = &result.domain_evidence[0];
        assert_eq!(domain.successful_routes.len(), 4);
    }

    #[test]
    fn uses_max_timestamp() {
        let episodes = vec![
            make_episode(
                "e1",
                Some("scrape"),
                &["navigate"],
                &["example.com"],
                &["navigate"],
                MemoryEpisodeResult::Success,
                1000,
                true,
            ),
            make_episode(
                "e2",
                Some("scrape"),
                &["navigate"],
                &["example.com"],
                &["navigate"],
                MemoryEpisodeResult::Success,
                5000,
                true,
            ),
            make_episode(
                "e3",
                Some("scrape"),
                &["navigate"],
                &["example.com"],
                &["navigate"],
                MemoryEpisodeResult::Success,
                3000,
                true,
            ),
        ];

        let result =
            aggregate_evidence_from_episodes(&episodes, EvidenceAggregationConfig::default());

        assert_eq!(result.task_evidence[0].last_used_epoch_secs, 5000);
        assert_eq!(result.domain_evidence[0].last_verified_epoch_secs, 5000);
    }

    #[test]
    fn returns_stable_sorted_output() {
        let episodes = vec![
            make_episode(
                "e1",
                Some("zebra"),
                &["navigate"],
                &["z.com"],
                &["navigate"],
                MemoryEpisodeResult::Success,
                1000,
                true,
            ),
            make_episode(
                "e2",
                Some("alpha"),
                &["navigate"],
                &["a.com"],
                &["navigate"],
                MemoryEpisodeResult::Success,
                2000,
                true,
            ),
            make_episode(
                "e3",
                Some("middle"),
                &["navigate"],
                &["m.com"],
                &["navigate"],
                MemoryEpisodeResult::Success,
                1500,
                true,
            ),
        ];

        let result =
            aggregate_evidence_from_episodes(&episodes, EvidenceAggregationConfig::default());

        assert_eq!(result.task_evidence.len(), 3);
        assert_eq!(result.task_evidence[0].task_class, "alpha");
        assert_eq!(result.task_evidence[1].task_class, "middle");
        assert_eq!(result.task_evidence[2].task_class, "zebra");

        assert_eq!(result.domain_evidence.len(), 3);
        assert_eq!(result.domain_evidence[0].domain, "a.com");
        assert_eq!(result.domain_evidence[1].domain, "m.com");
        assert_eq!(result.domain_evidence[2].domain, "z.com");
    }

    #[test]
    fn empty_input_returns_empty_result() {
        let result = aggregate_evidence_from_episodes(&[], EvidenceAggregationConfig::default());

        assert!(result.task_evidence.is_empty());
        assert!(result.domain_evidence.is_empty());
    }
}
