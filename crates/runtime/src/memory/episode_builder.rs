use super::{MemoryEpisode, MemoryEpisodeResult};
use crate::{ContentBlock, ConversationMessage};
use std::collections::HashSet;

type RouteStep = (String, Option<String>);
type EpisodeExtraction = (Vec<String>, Vec<RouteStep>, Vec<String>);

pub struct MemoryEpisodeBuildInput<'a> {
    pub id: String,
    pub task_class: Option<String>,
    pub user_goal: String,
    pub result: MemoryEpisodeResult,
    pub output_summary: Option<String>,
    pub messages: &'a [ConversationMessage],
    pub created_at_epoch_secs: u64,
    pub promote_candidate: bool,
}

#[derive(Copy, Clone)]
pub struct MemoryEpisodeBuildConfig {
    pub max_route_steps: usize,
    pub max_domains: usize,
    pub max_tools: usize,
    pub max_output_summary_chars: usize,
}

impl Default for MemoryEpisodeBuildConfig {
    fn default() -> Self {
        Self {
            max_route_steps: 32,
            max_domains: 16,
            max_tools: 16,
            max_output_summary_chars: 500,
        }
    }
}

#[must_use]
pub fn build_memory_episode(
    input: MemoryEpisodeBuildInput<'_>,
    config: MemoryEpisodeBuildConfig,
) -> MemoryEpisode {
    let output_summary = input
        .output_summary
        .map(|s| truncate_to_chars(&s, config.max_output_summary_chars));

    let (tool_names, route_steps, all_domains) = collect_tools_routes_and_domains(input.messages);

    let tools: Vec<String> = collect_preserving_order(tool_names.iter(), config.max_tools);

    let domains: Vec<String> = collect_preserving_order(all_domains.iter(), config.max_domains);

    let route: Vec<String> = route_steps
        .into_iter()
        .map(|(name, host)| match host {
            Some(h) => format!("{name}: {h}"),
            None => name,
        })
        .take(config.max_route_steps)
        .collect();

    MemoryEpisode {
        id: input.id,
        task_class: input.task_class,
        user_goal: input.user_goal,
        route,
        domains,
        tools,
        result: input.result,
        output_summary,
        created_at_epoch_secs: input.created_at_epoch_secs,
        promote_candidate: input.promote_candidate,
    }
}

fn truncate_to_chars(s: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    if s.chars().count() <= max {
        return s.to_string();
    }
    let byte_end = s
        .char_indices()
        .take(max)
        .last()
        .map_or(0, |(idx, ch)| idx + ch.len_utf8());
    s[..byte_end].to_string()
}

fn collect_tools_routes_and_domains(messages: &[ConversationMessage]) -> EpisodeExtraction {
    let mut tool_names: Vec<String> = Vec::new();
    let mut route_steps: Vec<RouteStep> = Vec::new();
    let mut all_domains: Vec<String> = Vec::new();

    for msg in messages {
        for block in &msg.blocks {
            match block {
                ContentBlock::Text { text } => {
                    for host in extract_hosts(text) {
                        push_unique(&mut all_domains, host);
                    }
                }
                ContentBlock::ToolUse { name, input, .. } => {
                    let hosts = extract_hosts(input);
                    let primary_host = hosts.first().cloned();
                    tool_names.push(name.clone());
                    route_steps.push((name.clone(), primary_host));
                    for host in hosts {
                        push_unique(&mut all_domains, host);
                    }
                }
                ContentBlock::ToolResult {
                    tool_name, output, ..
                } => {
                    tool_names.push(tool_name.clone());
                    for host in extract_hosts(output) {
                        push_unique(&mut all_domains, host);
                    }
                }
                ContentBlock::Reasoning { .. } => {}
            }
        }
    }

    (tool_names, route_steps, all_domains)
}

fn push_unique(vec: &mut Vec<String>, item: String) {
    if !vec.contains(&item) {
        vec.push(item);
    }
}

fn collect_preserving_order<'a, I, S>(items: I, limit: usize) -> Vec<String>
where
    I: IntoIterator<Item = &'a S>,
    S: ToString + 'a,
{
    let mut seen: HashSet<String> = HashSet::new();
    let mut result: Vec<String> = Vec::new();
    for item in items {
        let s = item.to_string();
        if seen.insert(s.clone()) {
            result.push(s);
            if result.len() >= limit {
                break;
            }
        }
    }
    result
}

fn extract_hosts(text: &str) -> Vec<String> {
    let lower = text.to_lowercase();
    let mut hosts = Vec::new();
    let mut i = 0;
    let bytes = lower.as_bytes();

    while i < bytes.len() {
        let (skip, found_host) = find_next_host(bytes, i);

        if let Some(host) = found_host {
            if !host.is_empty() {
                hosts.push(host);
            }
        }

        if skip == 0 {
            i += 1;
        } else {
            i = skip;
        }
    }

    hosts
}

fn find_next_host(bytes: &[u8], start: usize) -> (usize, Option<String>) {
    let remaining = &bytes[start..];

    let scheme_len: usize = if remaining.starts_with(b"https://") {
        8
    } else if remaining.starts_with(b"http://") {
        7
    } else {
        return (0, None);
    };

    let host_start = start + scheme_len;
    let rest = &bytes[host_start..];

    if rest.is_empty() {
        return (host_start, None);
    }

    let host_body_end = rest
        .iter()
        .position(|&b| is_host_boundary(b))
        .unwrap_or(rest.len());

    let host = std::str::from_utf8(&rest[..host_body_end])
        .unwrap_or("")
        .to_string();

    (host_start + host_body_end, Some(host))
}

const fn is_host_boundary(b: u8) -> bool {
    matches!(
        b,
        b'/' | b'?'
            | b'#'
            | b','
            | b';'
            | b' '
            | b'\t'
            | b'\n'
            | b'\r'
            | b'"'
            | b'\''
            | b')'
            | b']'
            | b'>'
            | b'}'
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::MemoryEpisodeResult;
    use crate::{ContentBlock, ConversationMessage};

    fn make_text_block(text: &str) -> ContentBlock {
        ContentBlock::Text {
            text: text.to_string(),
        }
    }

    fn make_tool_use(name: &str, input: &str) -> ContentBlock {
        ContentBlock::ToolUse {
            id: format!("tu_{name}"),
            name: name.to_string(),
            input: input.to_string(),
        }
    }

    fn make_tool_result(tool_use_id: &str, tool_name: &str, output: &str) -> ContentBlock {
        ContentBlock::ToolResult {
            tool_use_id: tool_use_id.to_string(),
            tool_name: tool_name.to_string(),
            output: output.to_string(),
            is_error: false,
        }
    }

    fn assistant_msg(blocks: Vec<ContentBlock>) -> ConversationMessage {
        ConversationMessage::assistant(blocks)
    }

    fn user_msg(text: &str) -> ConversationMessage {
        ConversationMessage::user_text(text)
    }

    fn basic_input<'a>(
        id: &str,
        messages: &'a [ConversationMessage],
    ) -> MemoryEpisodeBuildInput<'a> {
        MemoryEpisodeBuildInput {
            id: id.to_string(),
            task_class: None,
            user_goal: "test goal".to_string(),
            result: MemoryEpisodeResult::Success,
            output_summary: None,
            messages,
            created_at_epoch_secs: 1000,
            promote_candidate: false,
        }
    }

    #[test]
    fn builds_episode_with_copied_scalar_fields() {
        let messages = vec![];
        let input = MemoryEpisodeBuildInput {
            id: "ep1".to_string(),
            task_class: Some("scrape".to_string()),
            user_goal: "extract titles".to_string(),
            result: MemoryEpisodeResult::Partial,
            output_summary: Some("got 3 titles".to_string()),
            messages: &messages,
            created_at_epoch_secs: 1_700_000_000,
            promote_candidate: true,
        };

        let episode = build_memory_episode(input, MemoryEpisodeBuildConfig::default());

        assert_eq!(episode.id, "ep1");
        assert_eq!(episode.task_class.as_deref(), Some("scrape"));
        assert_eq!(episode.user_goal, "extract titles");
        assert_eq!(episode.result, MemoryEpisodeResult::Partial);
        assert_eq!(episode.output_summary.as_deref(), Some("got 3 titles"));
        assert_eq!(episode.created_at_epoch_secs, 1_700_000_000);
        assert!(episode.promote_candidate);
    }

    #[test]
    fn extracts_tools_from_tool_use_and_tool_result_with_stable_dedupe_order() {
        let messages = vec![
            assistant_msg(vec![
                make_tool_use("navigate", "https://example.com"),
                make_tool_use("click", ""),
            ]),
            assistant_msg(vec![make_tool_result(
                "tu_read_content",
                "read_content",
                "read https://example.com/article",
            )]),
            assistant_msg(vec![make_tool_use("navigate", "https://other.com")]),
        ];

        let episode = build_memory_episode(
            basic_input("ep1", &messages),
            MemoryEpisodeBuildConfig::default(),
        );

        assert_eq!(episode.tools, vec!["navigate", "click", "read_content"]);
    }

    #[test]
    fn extracts_domains_from_text_tool_input_tool_output() {
        let messages = vec![
            user_msg("go to https://example.com and also https://docs.example.com/page"),
            assistant_msg(vec![make_tool_use(
                "navigate",
                "{\"url\": \"https://shop.example.com\"}",
            )]),
            assistant_msg(vec![make_tool_use(
                "extract",
                "{\"selector\": \"a[href='https://third.com']\"}",
            )]),
        ];

        let episode = build_memory_episode(
            basic_input("ep1", &messages),
            MemoryEpisodeBuildConfig::default(),
        );

        assert!(episode.domains.contains(&"example.com".to_string()));
        assert!(episode.domains.contains(&"docs.example.com".to_string()));
        assert!(episode.domains.contains(&"shop.example.com".to_string()));
        assert!(episode.domains.contains(&"third.com".to_string()));
    }

    #[test]
    fn route_records_tool_names_and_url_hosts_without_full_input() {
        let messages = vec![assistant_msg(vec![
            make_tool_use("navigate", "https://example.com/path?q=1"),
            make_tool_use("click", ""),
            make_tool_use("extract", "https://other.com"),
        ])];

        let episode = build_memory_episode(
            basic_input("ep1", &messages),
            MemoryEpisodeBuildConfig::default(),
        );

        assert_eq!(episode.route.len(), 3);
        assert_eq!(episode.route[0], "navigate: example.com");
        assert_eq!(episode.route[1], "click");
        assert_eq!(episode.route[2], "extract: other.com");
    }

    #[test]
    fn respects_max_route_steps() {
        let mut blocks = Vec::new();
        for i in 0..20 {
            blocks.push(make_tool_use(&format!("tool{i}"), ""));
        }
        let messages = vec![assistant_msg(blocks)];

        let config = MemoryEpisodeBuildConfig {
            max_route_steps: 5,
            ..MemoryEpisodeBuildConfig::default()
        };
        let episode = build_memory_episode(basic_input("ep1", &messages), config);

        assert_eq!(episode.route.len(), 5);
        for i in 0..5 {
            assert_eq!(episode.route[i], format!("tool{i}"));
        }
    }

    #[test]
    fn respects_max_domains_and_max_tools() {
        let blocks: Vec<_> = (0..20)
            .map(|i| {
                make_tool_use(
                    &format!("tool{i}"),
                    &format!("https://site{i}.example.com/page"),
                )
            })
            .collect();
        let messages = vec![assistant_msg(blocks)];

        let config = MemoryEpisodeBuildConfig {
            max_tools: 3,
            max_domains: 3,
            ..MemoryEpisodeBuildConfig::default()
        };
        let episode = build_memory_episode(basic_input("ep1", &messages), config);

        assert_eq!(episode.tools.len(), 3);
        assert_eq!(episode.domains.len(), 3);
    }

    #[test]
    fn truncates_output_summary_by_chars() {
        let messages = vec![];
        let input = MemoryEpisodeBuildInput {
            id: "ep1".to_string(),
            task_class: None,
            user_goal: "goal".to_string(),
            result: MemoryEpisodeResult::Success,
            output_summary: Some("ABCDEFGHIJ".to_string()),
            messages: &messages,
            created_at_epoch_secs: 1000,
            promote_candidate: false,
        };

        let config = MemoryEpisodeBuildConfig {
            max_output_summary_chars: 5,
            ..MemoryEpisodeBuildConfig::default()
        };
        let episode = build_memory_episode(input, config);

        assert_eq!(episode.output_summary.as_deref(), Some("ABCDE"));
    }

    #[test]
    fn handles_messages_with_no_tools_or_urls() {
        let messages = vec![
            user_msg("hello world"),
            assistant_msg(vec![make_text_block("ok")]),
        ];

        let episode = build_memory_episode(
            basic_input("ep1", &messages),
            MemoryEpisodeBuildConfig::default(),
        );

        assert!(episode.tools.is_empty());
        assert!(episode.domains.is_empty());
        assert!(episode.route.is_empty());
    }

    #[test]
    fn host_extraction_ignores_invalid_hosts() {
        let hosts = extract_hosts("http://");
        assert!(hosts.is_empty());

        let hosts = extract_hosts("https://");
        assert!(hosts.is_empty());

        let hosts = extract_hosts("ftp://example.com");
        assert!(hosts.is_empty());

        let hosts = extract_hosts("no url here");
        assert!(hosts.is_empty());
    }

    #[test]
    fn host_extraction_handles_boundaries() {
        let hosts = extract_hosts("https://example.com/path?a=1#frag");
        assert_eq!(hosts, vec!["example.com"]);

        let hosts = extract_hosts("http://example.com/page https://other.com/x");
        assert_eq!(hosts, vec!["example.com", "other.com"]);

        let hosts = extract_hosts("https://example.com, https://other.com; done");
        assert_eq!(hosts, vec!["example.com", "other.com"]);

        let hosts = extract_hosts("before (https://example.com) after");
        assert_eq!(hosts, vec!["example.com"]);

        let hosts = extract_hosts("\"http://example.com\"");
        assert_eq!(hosts, vec!["example.com"]);

        let hosts = extract_hosts("text http://a.com/path\nhttps://b.com");
        assert_eq!(hosts, vec!["a.com", "b.com"]);
    }

    #[test]
    fn host_extraction_lowercases() {
        let hosts = extract_hosts("HTTPS://Example.COM/Page");
        assert_eq!(hosts, vec!["example.com"]);
    }

    #[test]
    fn route_preserves_repeated_tool_steps() {
        let messages = vec![
            assistant_msg(vec![make_tool_use("navigate", "https://first.com")]),
            assistant_msg(vec![make_tool_use("navigate", "https://second.com")]),
            assistant_msg(vec![make_tool_use("click", "")]),
        ];

        let episode = build_memory_episode(
            basic_input("ep1", &messages),
            MemoryEpisodeBuildConfig::default(),
        );

        assert_eq!(episode.route.len(), 3);
        assert_eq!(episode.route[0], "navigate: first.com");
        assert_eq!(episode.route[1], "navigate: second.com");
        assert_eq!(episode.route[2], "click");
        assert_eq!(episode.tools.len(), 2);
    }

    #[test]
    fn domains_deduplicate_across_sources() {
        let messages = vec![
            user_msg("visit https://dup.com"),
            assistant_msg(vec![make_tool_use("navigate", "https://dup.com/page")]),
            assistant_msg(vec![make_tool_result(
                "tu_navigate",
                "navigate",
                "opened https://dup.com",
            )]),
        ];

        let episode = build_memory_episode(
            basic_input("ep1", &messages),
            MemoryEpisodeBuildConfig::default(),
        );

        let dup_count = episode
            .domains
            .iter()
            .filter(|d| d.as_str() == "dup.com")
            .count();
        assert_eq!(dup_count, 1);
    }

    #[test]
    fn truncate_to_zero_chars_returns_empty() {
        assert_eq!(truncate_to_chars("hello", 0), "");
    }

    #[test]
    fn truncate_to_exact_length_returns_full() {
        assert_eq!(truncate_to_chars("hello", 5), "hello");
        assert_eq!(truncate_to_chars("hello", 10), "hello");
    }

    #[test]
    fn truncate_handles_multibyte_chars() {
        assert_eq!(truncate_to_chars("héllo", 5), "héllo");
        assert_eq!(truncate_to_chars("héllo wörld", 7), "héllo w");
    }
}
