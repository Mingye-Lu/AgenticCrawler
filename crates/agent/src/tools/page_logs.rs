use std::collections::{HashMap, HashSet};

use browser::{ConsoleMessageEvent, ConsoleMessageType, ObservationEvent};
use serde_json::{json, Value};

use crate::BrowserContext;
use crate::{CrawlError, CrawlState, ToolEffect, ToolExecutionError};

const DEFAULT_INSPECT_LIMIT: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LogLevelFilter {
    All,
    Error,
    Warning,
    Info,
    Debug,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GroupBy {
    Message,
    Source,
    Level,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SeqBound {
    All,
    Last,
    Now,
    Seq(u64),
}

#[derive(Debug)]
struct ListPageLogsInput {
    level: LogLevelFilter,
    since: SeqBound,
    until: SeqBound,
    group_by: GroupBy,
}

#[derive(Debug)]
struct InspectLogInput {
    id: String,
    limit: usize,
}

#[derive(Debug, Clone)]
struct MessageGroup {
    key: String,
    level: String,
    source: Option<String>,
    events: Vec<ConsoleMessageEvent>,
    first_at_ms: u64,
    last_at_ms: u64,
}

#[derive(Debug, Clone)]
struct SourceGroup {
    key: String,
    count: usize,
    breakdown: HashMap<String, usize>,
}

#[derive(Debug, Clone)]
struct LevelGroup {
    level: String,
    count: usize,
    top_message: String,
    sources: usize,
}

fn parse_list_input(input: &Value) -> Result<ListPageLogsInput, CrawlError> {
    let level = match input.get("level") {
        None | Some(Value::Null) => LogLevelFilter::All,
        Some(Value::String(level)) => match level.as_str() {
            "all" => LogLevelFilter::All,
            "error" => LogLevelFilter::Error,
            "warning" => LogLevelFilter::Warning,
            "info" => LogLevelFilter::Info,
            "debug" => LogLevelFilter::Debug,
            _ => {
                return Err(CrawlError::new(
                    "level must be one of: all, error, warning, info, debug",
                ));
            }
        },
        _ => return Err(CrawlError::new("level must be a string")),
    };

    let since = parse_seq_bound(input.get("since"), true)?;
    let until = parse_seq_bound(input.get("until"), false)?;

    let group_by = match input.get("group_by") {
        None | Some(Value::Null) => GroupBy::Message,
        Some(Value::String(group_by)) => match group_by.as_str() {
            "message" => GroupBy::Message,
            "source" => GroupBy::Source,
            "level" => GroupBy::Level,
            _ => {
                return Err(CrawlError::new(
                    "group_by must be one of: message, source, level",
                ));
            }
        },
        _ => return Err(CrawlError::new("group_by must be a string")),
    };

    Ok(ListPageLogsInput {
        level,
        since,
        until,
        group_by,
    })
}

fn parse_inspect_input(input: &Value) -> Result<InspectLogInput, CrawlError> {
    let id = input
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| CrawlError::new("missing required field: id"))?;
    if id.is_empty() {
        return Err(CrawlError::new("id must not be empty"));
    }

    let limit = match input.get("limit") {
        None | Some(Value::Null) => DEFAULT_INSPECT_LIMIT,
        Some(Value::Number(number)) => {
            let raw = number
                .as_u64()
                .ok_or_else(|| CrawlError::new("limit must be a non-negative integer"))?;
            usize::try_from(raw).map_err(|_| CrawlError::new("limit is too large"))?
        }
        _ => return Err(CrawlError::new("limit must be an integer")),
    };

    Ok(InspectLogInput {
        id: id.to_string(),
        limit,
    })
}

fn parse_seq_bound(value: Option<&Value>, is_since: bool) -> Result<SeqBound, CrawlError> {
    match value {
        None | Some(Value::Null) => Ok(if is_since {
            SeqBound::Last
        } else {
            SeqBound::Now
        }),
        Some(Value::String(raw)) => match raw.as_str() {
            "all" if is_since => Ok(SeqBound::All),
            "last" if is_since => Ok(SeqBound::Last),
            "now" if !is_since => Ok(SeqBound::Now),
            _ if is_since => Err(CrawlError::new(
                "since must be 'all', 'last', or a seq number",
            )),
            _ => Err(CrawlError::new("until must be 'now' or a seq number")),
        },
        Some(Value::Number(number)) => number
            .as_u64()
            .map(SeqBound::Seq)
            .ok_or_else(|| CrawlError::new("seq bounds must be non-negative integers")),
        _ if is_since => Err(CrawlError::new("since must be a string or number")),
        _ => Err(CrawlError::new("until must be a string or number")),
    }
}

fn normalized_level(level: &str) -> &'static str {
    match level.to_ascii_lowercase().as_str() {
        "error" => "error",
        "warn" | "warning" => "warning",
        "debug" | "trace" => "debug",
        _ => "info",
    }
}

fn level_matches(filter: LogLevelFilter, event: &ConsoleMessageEvent) -> bool {
    match filter {
        LogLevelFilter::All => true,
        LogLevelFilter::Error => normalized_level(&event.level) == "error",
        LogLevelFilter::Warning => normalized_level(&event.level) == "warning",
        LogLevelFilter::Info => normalized_level(&event.level) == "info",
        LogLevelFilter::Debug => normalized_level(&event.level) == "debug",
    }
}

fn source_label(event: &ConsoleMessageEvent) -> Option<String> {
    let raw = event.source_url.as_deref()?;
    let trimmed = raw
        .split('?')
        .next()
        .unwrap_or(raw)
        .split('#')
        .next()
        .unwrap_or(raw);
    let basename = trimmed
        .rsplit('/')
        .next()
        .filter(|segment| !segment.is_empty())
        .unwrap_or(trimmed);

    let mut label = basename.to_string();
    if let Some(line) = event.source_line {
        label.push(':');
        label.push_str(&line.to_string());
        if let Some(column) = event.source_column {
            label.push(':');
            label.push_str(&column.to_string());
        }
    }

    Some(label)
}

fn resolve_since(bound: SeqBound, state: &CrawlState) -> u64 {
    match bound {
        SeqBound::All | SeqBound::Now => 0,
        SeqBound::Last => state.seq_counter.current().saturating_sub(1),
        SeqBound::Seq(seq) => seq,
    }
}

fn resolve_until(bound: SeqBound) -> Option<u64> {
    match bound {
        SeqBound::Seq(seq) => Some(seq),
        SeqBound::Now | SeqBound::All | SeqBound::Last => None,
    }
}

fn drain_console_events(events: Vec<ObservationEvent>) -> Vec<ConsoleMessageEvent> {
    events
        .into_iter()
        .filter_map(|event| match event {
            ObservationEvent::ConsoleMessage(console) => Some(console),
            ObservationEvent::NetworkRequest(_) | ObservationEvent::WebSocketFrame(_) => None,
        })
        .collect()
}

fn matching_logs(state: &CrawlState, input: &ListPageLogsInput) -> Vec<ConsoleMessageEvent> {
    let since = resolve_since(input.since, state);
    let until = resolve_until(input.until);

    state
        .page_log_events
        .iter()
        .filter(|event| {
            event.seq_at_initiation >= since
                && until.is_none_or(|upper_bound| event.seq_at_initiation < upper_bound)
                && level_matches(input.level, event)
        })
        .cloned()
        .collect()
}

fn build_summary(events: &[ConsoleMessageEvent]) -> Value {
    let mut unique_messages = HashSet::new();
    let mut errors = 0;
    let mut warnings = 0;
    let mut info = 0;
    let mut debug = 0;

    for event in events {
        unique_messages.insert(event.text.as_str());
        match normalized_level(&event.level) {
            "error" => errors += 1,
            "warning" => warnings += 1,
            "debug" => debug += 1,
            _ => info += 1,
        }
    }

    json!({
        "total": events.len(),
        "unique": unique_messages.len(),
        "errors": errors,
        "warnings": warnings,
        "info": info,
        "debug": debug,
    })
}

fn group_by_message(events: &[ConsoleMessageEvent]) -> Vec<MessageGroup> {
    let mut grouped: HashMap<String, MessageGroup> = HashMap::new();

    for event in events {
        let entry = grouped
            .entry(event.text.clone())
            .or_insert_with(|| MessageGroup {
                key: event.text.clone(),
                level: normalized_level(&event.level).to_string(),
                source: source_label(event),
                events: Vec::new(),
                first_at_ms: event.timestamp_ms,
                last_at_ms: event.timestamp_ms,
            });
        entry.first_at_ms = entry.first_at_ms.min(event.timestamp_ms);
        entry.last_at_ms = entry.last_at_ms.max(event.timestamp_ms);
        entry.events.push(event.clone());
    }

    let mut groups: Vec<_> = grouped.into_values().collect();
    groups.sort_by(|left, right| {
        right
            .events
            .len()
            .cmp(&left.events.len())
            .then_with(|| left.key.cmp(&right.key))
    });
    groups
}

fn group_by_source(events: &[ConsoleMessageEvent]) -> Vec<SourceGroup> {
    let mut grouped: HashMap<String, SourceGroup> = HashMap::new();

    for event in events {
        let source = source_label(event).unwrap_or_else(|| "unknown".to_string());
        let level = normalized_level(&event.level).to_string();
        let entry = grouped
            .entry(source.clone())
            .or_insert_with(|| SourceGroup {
                key: source,
                count: 0,
                breakdown: HashMap::new(),
            });
        entry.count += 1;
        *entry.breakdown.entry(level).or_insert(0) += 1;
    }

    let mut groups: Vec<_> = grouped.into_values().collect();
    groups.sort_by(|left, right| {
        right
            .count
            .cmp(&left.count)
            .then_with(|| left.key.cmp(&right.key))
    });
    groups
}

fn group_by_level(events: &[ConsoleMessageEvent]) -> Vec<LevelGroup> {
    let mut grouped: HashMap<String, Vec<&ConsoleMessageEvent>> = HashMap::new();
    for event in events {
        grouped
            .entry(normalized_level(&event.level).to_string())
            .or_default()
            .push(event);
    }

    let mut groups = Vec::new();
    for (level, level_events) in grouped {
        let mut message_counts: HashMap<&str, usize> = HashMap::new();
        let mut sources = HashSet::new();
        for event in &level_events {
            *message_counts.entry(event.text.as_str()).or_insert(0) += 1;
            if let Some(source) = source_label(event) {
                sources.insert(source);
            }
        }

        let top_message = message_counts
            .into_iter()
            .max_by(|left, right| left.1.cmp(&right.1).then_with(|| right.0.cmp(left.0)))
            .map_or_else(String::new, |(message, _)| message.to_string());

        groups.push(LevelGroup {
            level,
            count: level_events.len(),
            top_message,
            sources: sources.len(),
        });
    }

    groups.sort_by(|left, right| {
        right
            .count
            .cmp(&left.count)
            .then_with(|| left.level.cmp(&right.level))
    });
    groups
}

fn console_message_type_name(message_type: ConsoleMessageType) -> &'static str {
    match message_type {
        ConsoleMessageType::Console => "console",
        ConsoleMessageType::Exception => "exception",
        ConsoleMessageType::PromiseRejection => "promise_rejection",
        ConsoleMessageType::CspViolation => "csp_violation",
    }
}

fn inspect_payload(events: &[ConsoleMessageEvent], limit: usize) -> Value {
    let Some(first) = events.first() else {
        return json!({
            "message": "",
            "level": "info",
            "type": "console",
            "total_occurrences": 0,
            "instances": [],
        });
    };

    let instances: Vec<_> = events
        .iter()
        .take(limit)
        .map(|event| {
            json!({
                "timestamp_ms": event.timestamp_ms,
                "stack": event.stack,
                "source": {
                    "file": event.source_url,
                    "line": event.source_line,
                    "column": event.source_column,
                }
            })
        })
        .collect();

    json!({
        "message": first.text,
        "level": normalized_level(&first.level),
        "type": console_message_type_name(first.message_type),
        "total_occurrences": events.len(),
        "instances": instances,
    })
}

pub async fn execute_list_page_logs(
    input: &Value,
    browser: &mut BrowserContext,
    crawl_state: &mut CrawlState,
) -> Result<ToolEffect, ToolExecutionError> {
    let params = parse_list_input(input)?;
    let observations = browser
        .acquire_bridge()
        .await
        .map_err(|error| ToolExecutionError::new(error.to_string()))?
        .poll_observations()
        .await
        .map_err(|error| ToolExecutionError::new(error.to_string()))?;

    crawl_state
        .page_log_events
        .extend(drain_console_events(observations));

    let logs = matching_logs(crawl_state, &params);
    crawl_state.last_page_log_seq = Some(crawl_state.seq_counter.current());

    let summary = build_summary(&logs);

    let response = match params.group_by {
        GroupBy::Message => {
            let groups = group_by_message(&logs);
            crawl_state.page_log_groups = groups
                .iter()
                .enumerate()
                .map(|(index, group)| (format!("@log{}", index + 1), group.events.clone()))
                .collect();

            let payload_groups: Vec<_> = groups
                .into_iter()
                .enumerate()
                .map(|(index, group)| {
                    json!({
                        "id": format!("@log{}", index + 1),
                        "level": group.level,
                        "message": group.key,
                        "count": group.events.len(),
                        "source": group.source,
                        "first_at_ms": group.first_at_ms,
                        "last_at_ms": group.last_at_ms,
                    })
                })
                .collect();

            json!({
                "summary": summary,
                "groups": payload_groups,
                "truncated": false,
            })
        }
        GroupBy::Source => {
            crawl_state.page_log_groups.clear();
            let payload_groups: Vec<_> = group_by_source(&logs)
                .into_iter()
                .enumerate()
                .map(|(index, group)| {
                    json!({
                        "id": format!("@src{}", index + 1),
                        "source": group.key,
                        "count": group.count,
                        "breakdown": {
                            "error": group.breakdown.get("error").copied().unwrap_or(0),
                            "warning": group.breakdown.get("warning").copied().unwrap_or(0),
                            "info": group.breakdown.get("info").copied().unwrap_or(0),
                            "debug": group.breakdown.get("debug").copied().unwrap_or(0),
                        }
                    })
                })
                .collect();

            json!({
                "summary": summary,
                "groups": payload_groups,
                "truncated": false,
            })
        }
        GroupBy::Level => {
            crawl_state.page_log_groups.clear();
            let payload_groups: Vec<_> = group_by_level(&logs)
                .into_iter()
                .enumerate()
                .map(|(index, group)| {
                    json!({
                        "id": format!("@lvl{}", index + 1),
                        "level": group.level,
                        "count": group.count,
                        "top_message": group.top_message,
                        "sources": group.sources,
                    })
                })
                .collect();

            json!({
                "summary": summary,
                "groups": payload_groups,
                "truncated": false,
            })
        }
    };

    Ok(ToolEffect::reply_json(&response))
}

pub fn execute_inspect_log(
    input: &Value,
    crawl_state: &mut CrawlState,
) -> Result<ToolEffect, ToolExecutionError> {
    let params = parse_inspect_input(input)?;
    let events = crawl_state
        .page_log_groups
        .get(&params.id)
        .ok_or_else(|| ToolExecutionError::new(format!("unknown log group id: {}", params.id)))?;

    Ok(ToolEffect::reply_json(&inspect_payload(
        events,
        params.limit,
    )))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use browser::{
        BridgeError, BrowserBackend, BrowserState, PageInfo, ScreenshotOptions, SharedBridge,
        StorageEntry, StorageType,
    };
    use tokio::sync::Mutex;

    use super::*;

    #[derive(Debug, Default)]
    struct MockBackend {
        observations: Vec<ObservationEvent>,
    }

    #[async_trait]
    impl BrowserBackend for MockBackend {
        async fn navigate(&mut self, _: &str) -> Result<PageInfo, BridgeError> {
            Err(BridgeError::Protocol("unused".to_string()))
        }
        async fn new_page(&mut self, _: Option<&str>) -> Result<usize, BridgeError> {
            Ok(0)
        }
        async fn close_page(&mut self, _: usize) -> Result<(), BridgeError> {
            Ok(())
        }
        async fn scroll(&mut self, _: &str, _: i64) -> Result<(), BridgeError> {
            Ok(())
        }
        async fn page_map(&mut self, _: Option<&str>, _: bool) -> Result<Value, BridgeError> {
            Ok(json!({}))
        }
        async fn read_content(
            &mut self,
            _: Option<&str>,
            _: Option<&str>,
            _: usize,
            _: usize,
        ) -> Result<Value, BridgeError> {
            Ok(json!({}))
        }
        async fn wait_for_selector(
            &mut self,
            _: &str,
            _: u64,
            _: Option<&str>,
        ) -> Result<bool, BridgeError> {
            Ok(true)
        }
        async fn select_option(&mut self, _: &str, _: &str) -> Result<(), BridgeError> {
            Ok(())
        }
        async fn evaluate(&mut self, _: &str) -> Result<Value, BridgeError> {
            Ok(Value::Null)
        }
        async fn hover(&mut self, _: &str) -> Result<(), BridgeError> {
            Ok(())
        }
        async fn press_key(&mut self, _: &str, _: Option<&str>) -> Result<(), BridgeError> {
            Ok(())
        }
        async fn switch_tab(&mut self, _: i64) -> Result<Value, BridgeError> {
            Ok(json!({}))
        }
        async fn export_cookies(&mut self) -> Result<BrowserState, BridgeError> {
            Ok(BrowserState {
                cookies: Value::Array(vec![]),
                local_storage: Value::Object(serde_json::Map::new()),
                url: String::new(),
            })
        }
        async fn import_cookies(&mut self, _: &BrowserState) -> Result<(), BridgeError> {
            Ok(())
        }
        async fn import_cookies_only(&mut self, _: &BrowserState) -> Result<(), BridgeError> {
            Ok(())
        }
        async fn import_local_storage(&mut self, _: &BrowserState) -> Result<(), BridgeError> {
            Ok(())
        }
        async fn list_resources(&mut self) -> Result<Value, BridgeError> {
            Ok(json!([]))
        }
        async fn save_file(&mut self, _: &str, _: &str) -> Result<String, BridgeError> {
            Ok(String::new())
        }
        async fn click(&mut self, _: &str) -> Result<(), BridgeError> {
            Ok(())
        }
        async fn click_at(&mut self, _: f64, _: f64) -> Result<(), BridgeError> {
            Ok(())
        }
        async fn fill(&mut self, _: &str, _: &str) -> Result<(), BridgeError> {
            Ok(())
        }
        async fn screenshot(
            &mut self,
            _: &ScreenshotOptions<'_>,
        ) -> Result<(String, usize), BridgeError> {
            Ok((String::new(), 0))
        }
        async fn go_back(&mut self) -> Result<String, BridgeError> {
            Ok(String::new())
        }
        async fn set_device(&mut self, _: &Value) -> Result<Value, BridgeError> {
            Ok(json!({}))
        }
        async fn poll_observations(&mut self) -> Result<Vec<ObservationEvent>, BridgeError> {
            Ok(std::mem::take(&mut self.observations))
        }
        async fn get_storage(
            &mut self,
            _: StorageType,
        ) -> Result<(Vec<StorageEntry>, Vec<StorageEntry>), BridgeError> {
            Ok((Vec::new(), Vec::new()))
        }
    }

    fn make_browser(observations: Vec<ObservationEvent>) -> BrowserContext {
        let bridge: SharedBridge = Arc::new(Mutex::new(
            Box::new(MockBackend { observations }) as Box<dyn BrowserBackend + Send>
        ));
        BrowserContext::new(bridge)
    }

    fn console_event(
        seq: u64,
        level: &str,
        text: &str,
        timestamp_ms: u64,
        source_url: Option<&str>,
        stack: Option<&str>,
    ) -> ConsoleMessageEvent {
        ConsoleMessageEvent {
            timestamp_ms,
            tab_index: 0,
            seq_at_initiation: seq,
            level: level.to_string(),
            message_type: ConsoleMessageType::Exception,
            text: text.to_string(),
            source_url: source_url.map(str::to_string),
            source_line: Some(89),
            source_column: Some(12),
            stack: stack.map(str::to_string),
        }
    }

    #[test]
    fn parse_list_input_defaults() {
        let parsed = parse_list_input(&json!({})).unwrap();
        assert_eq!(parsed.level, LogLevelFilter::All);
        assert_eq!(parsed.since, SeqBound::Last);
        assert_eq!(parsed.until, SeqBound::Now);
        assert_eq!(parsed.group_by, GroupBy::Message);
    }

    #[test]
    fn filter_console_events_respects_bounds_and_level() {
        let events = vec![
            ObservationEvent::ConsoleMessage(console_event(1, "info", "a", 10, None, None)),
            ObservationEvent::ConsoleMessage(console_event(3, "warn", "b", 20, None, None)),
            ObservationEvent::ConsoleMessage(console_event(5, "error", "c", 30, None, None)),
        ];
        let input = ListPageLogsInput {
            level: LogLevelFilter::Warning,
            since: SeqBound::Seq(2),
            until: SeqBound::Seq(5),
            group_by: GroupBy::Message,
        };

        let state = CrawlState {
            page_log_events: drain_console_events(events),
            ..CrawlState::default()
        };
        let filtered = matching_logs(&state, &input);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].text, "b");
    }

    #[test]
    fn group_by_message_deduplicates_and_sorts_by_frequency() {
        let events = vec![
            console_event(
                1,
                "warn",
                "repeat",
                100,
                Some("https://cdn.test/react-dom.js"),
                None,
            ),
            console_event(
                2,
                "warn",
                "repeat",
                200,
                Some("https://cdn.test/react-dom.js"),
                None,
            ),
            console_event(
                3,
                "error",
                "once",
                300,
                Some("https://app.test/app.js"),
                None,
            ),
        ];

        let groups = group_by_message(&events);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].key, "repeat");
        assert_eq!(groups[0].events.len(), 2);
        assert_eq!(groups[0].first_at_ms, 100);
        assert_eq!(groups[0].last_at_ms, 200);
    }

    #[tokio::test]
    async fn list_page_logs_stores_log_groups_for_inspection() {
        let observations = vec![
            ObservationEvent::ConsoleMessage(console_event(
                4,
                "error",
                "TypeError",
                450,
                Some("https://app.test/app.js"),
                Some("TypeError\\n at App.render"),
            )),
            ObservationEvent::ConsoleMessage(console_event(
                4,
                "error",
                "TypeError",
                500,
                Some("https://app.test/app.js"),
                Some("TypeError\\n at App.render"),
            )),
            ObservationEvent::ConsoleMessage(console_event(
                5,
                "warn",
                "Deprecated API",
                700,
                Some("https://app.test/vendor.js"),
                None,
            )),
        ];
        let mut browser = make_browser(observations);
        let mut state = CrawlState::default();

        let effect =
            execute_list_page_logs(&json!({ "group_by": "message" }), &mut browser, &mut state)
                .await
                .unwrap();
        let rendered = format!("{effect:?}");

        assert!(rendered.contains("@log1"));
        assert!(state.page_log_groups.contains_key("@log1"));
        assert_eq!(state.page_log_groups["@log1"].len(), 2);
    }

    #[tokio::test]
    async fn inspect_log_returns_occurrence_details() {
        let mut state = CrawlState::default();
        state.page_log_groups.insert(
            "@log1".to_string(),
            vec![
                console_event(
                    4,
                    "error",
                    "TypeError: Cannot read properties of undefined",
                    450,
                    Some("app.js"),
                    Some("TypeError: ...\\n    at App.render (app.js:89:12)"),
                ),
                console_event(
                    4,
                    "error",
                    "TypeError: Cannot read properties of undefined",
                    500,
                    Some("app.js"),
                    Some("TypeError: ...\\n    at App.render (app.js:89:12)"),
                ),
            ],
        );

        let effect =
            execute_inspect_log(&json!({ "id": "@log1", "limit": 1 }), &mut state).unwrap();
        let rendered = format!("{effect:?}");

        assert!(rendered.contains("total_occurrences"));
        assert!(rendered.contains("TypeError: Cannot read properties of undefined"));
        assert!(rendered.contains("app.js"));
    }
}
