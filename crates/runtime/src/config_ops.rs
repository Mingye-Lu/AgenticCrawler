use std::fmt;
use std::io;
use std::path::PathBuf;

use serde_json::{json, Value};

use crate::settings::{
    load_settings, save_settings, settings_file_path, settings_get_action_cache_ttl_secs,
    settings_get_action_caching, settings_get_auto_compact_tokens, settings_get_budget_enforcement,
    settings_get_budget_max_session_cost_usd, settings_get_budget_warn_threshold_pct,
    settings_get_compaction_llm_summarization, settings_get_compaction_max_summary_chars,
    settings_get_compaction_preserve_recent_messages_floor,
    settings_get_compaction_preserve_recent_tokens, settings_get_compaction_prune_max_output_chars,
    settings_get_compaction_prune_protect_tokens, settings_get_compound_enrichment,
    settings_get_confidence_tracking, settings_get_content_aware_profiles,
    settings_get_failure_classification, settings_get_fork_child_max_steps,
    settings_get_fork_wait_timeout_secs, settings_get_headless, settings_get_html_diff_mode,
    settings_get_loop_detection, settings_get_loop_detection_window,
    settings_get_loop_nudge_threshold, settings_get_max_concurrent_per_parent,
    settings_get_max_fork_depth, settings_get_max_steps, settings_get_max_total_agents,
    settings_get_output_dir, settings_get_page_fingerprinting,
    settings_get_per_agent_cost_tracking, settings_get_planning_interval,
    settings_get_self_healing, settings_get_self_healing_max_retries, OptimizationSettings,
    ScriptSettings, Settings,
};

const REASONING_EFFORT_VALUES: &[&str] = &["high", "medium", "low"];
const BROWSER_BACKEND_VALUES: &[&str] = &["extension"];
const BUDGET_ENFORCEMENT_VALUES: &[&str] = &["warn", "block"];

#[derive(Debug)]
pub enum ConfigError {
    UnknownKey(String),
    BadValue {
        key: String,
        value: String,
        reason: String,
    },
    Io(io::Error),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownKey(key) => write!(f, "unknown config key: {key}"),
            Self::BadValue { key, value, reason } => {
                write!(f, "bad value for {key}: {value} ({reason})")
            }
            Self::Io(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            Self::UnknownKey(_) | Self::BadValue { .. } => None,
        }
    }
}

impl From<io::Error> for ConfigError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

#[derive(Clone, Copy, Debug)]
enum ValueKind {
    Bool,
    U16,
    U32,
    U64,
    Usize,
    String,
    Path,
    Enum(&'static [&'static str]),
    NullableEnum(&'static [&'static str]),
    NullableFloat,
    Model,
}

#[derive(Clone, Copy, Debug)]
struct KeySpec {
    key: &'static str,
    kind: ValueKind,
}

const KEY_SPECS: &[KeySpec] = &[
    KeySpec {
        key: "headless",
        kind: ValueKind::Bool,
    },
    KeySpec {
        key: "max_steps",
        kind: ValueKind::U32,
    },
    KeySpec {
        key: "model",
        kind: ValueKind::Model,
    },
    KeySpec {
        key: "reasoning_effort",
        kind: ValueKind::Enum(REASONING_EFFORT_VALUES),
    },
    KeySpec {
        key: "output_dir",
        kind: ValueKind::Path,
    },
    KeySpec {
        key: "auto_compact_input_tokens",
        kind: ValueKind::U64,
    },
    KeySpec {
        key: "max_concurrent_per_parent",
        kind: ValueKind::U32,
    },
    KeySpec {
        key: "max_fork_depth",
        kind: ValueKind::U32,
    },
    KeySpec {
        key: "max_total_agents",
        kind: ValueKind::U32,
    },
    KeySpec {
        key: "fork_child_max_steps",
        kind: ValueKind::U32,
    },
    KeySpec {
        key: "fork_wait_timeout_secs",
        kind: ValueKind::U32,
    },
    KeySpec {
        key: "extension_bridge_token",
        kind: ValueKind::String,
    },
    KeySpec {
        key: "extension_bridge_port",
        kind: ValueKind::U16,
    },
    KeySpec {
        key: "browser_backend",
        kind: ValueKind::NullableEnum(BROWSER_BACKEND_VALUES),
    },
    KeySpec {
        key: "compaction_prune_protect_tokens",
        kind: ValueKind::U64,
    },
    KeySpec {
        key: "compaction_prune_max_output_chars",
        kind: ValueKind::U64,
    },
    KeySpec {
        key: "compaction_preserve_recent_tokens",
        kind: ValueKind::U64,
    },
    KeySpec {
        key: "compaction_preserve_recent_messages_floor",
        kind: ValueKind::U32,
    },
    KeySpec {
        key: "compaction_max_summary_chars",
        kind: ValueKind::U64,
    },
    KeySpec {
        key: "compaction_llm_summarization",
        kind: ValueKind::Bool,
    },
    KeySpec {
        key: "optimization.html_diff_mode",
        kind: ValueKind::Bool,
    },
    KeySpec {
        key: "optimization.loop_detection",
        kind: ValueKind::Bool,
    },
    KeySpec {
        key: "optimization.loop_detection_window",
        kind: ValueKind::Usize,
    },
    KeySpec {
        key: "optimization.loop_nudge_threshold",
        kind: ValueKind::Usize,
    },
    KeySpec {
        key: "optimization.page_fingerprinting",
        kind: ValueKind::Bool,
    },
    KeySpec {
        key: "optimization.planning_interval",
        kind: ValueKind::Usize,
    },
    KeySpec {
        key: "optimization.failure_classification",
        kind: ValueKind::Bool,
    },
    KeySpec {
        key: "optimization.self_healing",
        kind: ValueKind::Bool,
    },
    KeySpec {
        key: "optimization.self_healing_max_retries",
        kind: ValueKind::Usize,
    },
    KeySpec {
        key: "optimization.action_caching",
        kind: ValueKind::Bool,
    },
    KeySpec {
        key: "optimization.action_cache_ttl_secs",
        kind: ValueKind::U64,
    },
    KeySpec {
        key: "optimization.confidence_tracking",
        kind: ValueKind::Bool,
    },
    KeySpec {
        key: "optimization.compound_enrichment",
        kind: ValueKind::Bool,
    },
    KeySpec {
        key: "optimization.budget_max_session_cost_usd",
        kind: ValueKind::NullableFloat,
    },
    KeySpec {
        key: "optimization.budget_enforcement",
        kind: ValueKind::NullableEnum(BUDGET_ENFORCEMENT_VALUES),
    },
    KeySpec {
        key: "optimization.budget_warn_threshold_pct",
        kind: ValueKind::U32,
    },
    KeySpec {
        key: "optimization.per_agent_cost_tracking",
        kind: ValueKind::Bool,
    },
    KeySpec {
        key: "optimization.content_aware_profiles",
        kind: ValueKind::Bool,
    },
    KeySpec {
        key: "script.max_steps",
        kind: ValueKind::Usize,
    },
    KeySpec {
        key: "script.max_timeout_secs",
        kind: ValueKind::U64,
    },
    KeySpec {
        key: "script.max_output_bytes",
        kind: ValueKind::Usize,
    },
    KeySpec {
        key: "script.max_parallel_branches",
        kind: ValueKind::Usize,
    },
    KeySpec {
        key: "script.max_concurrent_scripts",
        kind: ValueKind::Usize,
    },
    KeySpec {
        key: "script.per_step_timeout_secs",
        kind: ValueKind::U64,
    },
    KeySpec {
        key: "script.max_script_size_bytes",
        kind: ValueKind::Usize,
    },
    KeySpec {
        key: "script.max_nesting_depth",
        kind: ValueKind::Usize,
    },
    KeySpec {
        key: "script.scripts_dir",
        kind: ValueKind::Path,
    },
];

#[must_use]
pub fn config_path() -> PathBuf {
    settings_file_path()
}

pub fn config_set(key: &str, value: &str) -> Result<(), ConfigError> {
    let spec = lookup_key_spec(key).ok_or_else(|| ConfigError::UnknownKey(key.to_string()))?;
    let mut settings = load_settings();
    apply_set(&mut settings, spec, value)?;
    save_settings(&settings)?;
    Ok(())
}

pub fn config_get(key: &str, effective: bool) -> Result<String, ConfigError> {
    let settings = load_settings();
    if key.is_empty() {
        let json = if effective {
            serde_json::to_string_pretty(&effective_settings(&settings))
        } else {
            serde_json::to_string_pretty(&settings)
        };
        return json.map_err(json_error_to_io).map_err(ConfigError::Io);
    }

    let _ = lookup_key_spec(key).ok_or_else(|| ConfigError::UnknownKey(key.to_string()))?;
    let value = if effective {
        effective_value(&settings, key)
    } else {
        stored_value(&settings, key)
    };
    serde_json::to_string(&value)
        .map_err(json_error_to_io)
        .map_err(ConfigError::Io)
}

pub fn config_unset(key: &str) -> Result<(), ConfigError> {
    let mut settings = load_settings();
    match key {
        "optimization" => settings.optimization = None,
        "script" => settings.script = None,
        _ => {
            let _ = lookup_key_spec(key).ok_or_else(|| ConfigError::UnknownKey(key.to_string()))?;
            apply_unset(&mut settings, key);
            prune_empty_blocks(&mut settings);
        }
    }
    save_settings(&settings)?;
    Ok(())
}

fn json_error_to_io(err: serde_json::Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, err)
}

fn lookup_key_spec(key: &str) -> Option<&'static KeySpec> {
    KEY_SPECS.iter().find(|spec| spec.key == key)
}

fn bad_value(key: &str, value: &str, reason: impl Into<String>) -> ConfigError {
    ConfigError::BadValue {
        key: key.to_string(),
        value: value.to_string(),
        reason: reason.into(),
    }
}

fn parse_bool(key: &str, value: &str) -> Result<bool, ConfigError> {
    value
        .parse::<bool>()
        .map_err(|_| bad_value(key, value, "expected bool (true or false)"))
}

fn parse_u16(key: &str, value: &str) -> Result<u16, ConfigError> {
    value
        .parse::<u16>()
        .map_err(|_| bad_value(key, value, "expected unsigned integer"))
}

fn parse_u32(key: &str, value: &str) -> Result<u32, ConfigError> {
    value
        .parse::<u32>()
        .map_err(|_| bad_value(key, value, "expected unsigned integer"))
}

fn parse_u64(key: &str, value: &str) -> Result<u64, ConfigError> {
    value
        .parse::<u64>()
        .map_err(|_| bad_value(key, value, "expected unsigned integer"))
}

fn parse_usize(key: &str, value: &str) -> Result<usize, ConfigError> {
    value
        .parse::<usize>()
        .map_err(|_| bad_value(key, value, "expected unsigned integer"))
}

fn parse_float(key: &str, value: &str) -> Result<f64, ConfigError> {
    value
        .parse::<f64>()
        .map_err(|_| bad_value(key, value, "expected float"))
}

fn parse_enum<'a>(
    key: &str,
    value: &'a str,
    allowed: &'static [&'static str],
) -> Result<&'a str, ConfigError> {
    if allowed.contains(&value) {
        Ok(value)
    } else {
        Err(bad_value(
            key,
            value,
            format!("expected one of: {}", allowed.join(", ")),
        ))
    }
}

fn parse_nullable_enum(
    key: &str,
    value: &str,
    allowed: &'static [&'static str],
) -> Result<Option<String>, ConfigError> {
    if value == "null" {
        Ok(None)
    } else {
        parse_enum(key, value, allowed).map(|parsed| Some(parsed.to_string()))
    }
}

fn parse_nullable_float(key: &str, value: &str) -> Result<Option<f64>, ConfigError> {
    if value == "null" {
        Ok(None)
    } else {
        parse_float(key, value).map(Some)
    }
}

fn parse_model(key: &str, value: &str) -> Result<String, ConfigError> {
    if value.contains('/') {
        Ok(value.to_string())
    } else {
        Err(bad_value(
            key,
            value,
            "expected provider/model format containing '/'",
        ))
    }
}

fn apply_set(settings: &mut Settings, spec: &KeySpec, value: &str) -> Result<(), ConfigError> {
    match (spec.key, spec.kind) {
        ("headless", ValueKind::Bool) => settings.headless = Some(parse_bool(spec.key, value)?),
        ("max_steps", ValueKind::U32) => settings.max_steps = Some(parse_u32(spec.key, value)?),
        ("model", ValueKind::Model) => settings.model = Some(parse_model(spec.key, value)?),
        ("reasoning_effort", ValueKind::Enum(allowed)) => {
            settings.reasoning_effort = Some(parse_enum(spec.key, value, allowed)?.to_string())
        }
        ("output_dir", ValueKind::Path | ValueKind::String) => {
            settings.output_dir = Some(value.to_string())
        }
        ("auto_compact_input_tokens", ValueKind::U64) => {
            settings.auto_compact_input_tokens = Some(parse_u64(spec.key, value)?)
        }
        ("max_concurrent_per_parent", ValueKind::U32) => {
            settings.max_concurrent_per_parent = Some(parse_u32(spec.key, value)?)
        }
        ("max_fork_depth", ValueKind::U32) => {
            settings.max_fork_depth = Some(parse_u32(spec.key, value)?)
        }
        ("max_total_agents", ValueKind::U32) => {
            settings.max_total_agents = Some(parse_u32(spec.key, value)?)
        }
        ("fork_child_max_steps", ValueKind::U32) => {
            settings.fork_child_max_steps = Some(parse_u32(spec.key, value)?)
        }
        ("fork_wait_timeout_secs", ValueKind::U32) => {
            settings.fork_wait_timeout_secs = Some(parse_u32(spec.key, value)?)
        }
        ("extension_bridge_token", ValueKind::String) => {
            settings.extension_bridge_token = Some(value.to_string())
        }
        ("extension_bridge_port", ValueKind::U16) => {
            settings.extension_bridge_port = Some(parse_u16(spec.key, value)?)
        }
        ("browser_backend", ValueKind::NullableEnum(allowed)) => {
            settings.browser_backend = parse_nullable_enum(spec.key, value, allowed)?
        }
        ("compaction_prune_protect_tokens", ValueKind::U64) => {
            settings.compaction_prune_protect_tokens = Some(parse_u64(spec.key, value)?)
        }
        ("compaction_prune_max_output_chars", ValueKind::U64) => {
            settings.compaction_prune_max_output_chars = Some(parse_u64(spec.key, value)?)
        }
        ("compaction_preserve_recent_tokens", ValueKind::U64) => {
            settings.compaction_preserve_recent_tokens = Some(parse_u64(spec.key, value)?)
        }
        ("compaction_preserve_recent_messages_floor", ValueKind::U32) => {
            settings.compaction_preserve_recent_messages_floor = Some(parse_u32(spec.key, value)?)
        }
        ("compaction_max_summary_chars", ValueKind::U64) => {
            settings.compaction_max_summary_chars = Some(parse_u64(spec.key, value)?)
        }
        ("compaction_llm_summarization", ValueKind::Bool) => {
            settings.compaction_llm_summarization = Some(parse_bool(spec.key, value)?)
        }
        ("optimization.html_diff_mode", ValueKind::Bool) => {
            optimization_mut(settings).html_diff_mode = Some(parse_bool(spec.key, value)?)
        }
        ("optimization.loop_detection", ValueKind::Bool) => {
            optimization_mut(settings).loop_detection = Some(parse_bool(spec.key, value)?)
        }
        ("optimization.loop_detection_window", ValueKind::Usize) => {
            optimization_mut(settings).loop_detection_window = Some(parse_usize(spec.key, value)?)
        }
        ("optimization.loop_nudge_threshold", ValueKind::Usize) => {
            optimization_mut(settings).loop_nudge_threshold = Some(parse_usize(spec.key, value)?)
        }
        ("optimization.page_fingerprinting", ValueKind::Bool) => {
            optimization_mut(settings).page_fingerprinting = Some(parse_bool(spec.key, value)?)
        }
        ("optimization.planning_interval", ValueKind::Usize) => {
            optimization_mut(settings).planning_interval = Some(parse_usize(spec.key, value)?)
        }
        ("optimization.failure_classification", ValueKind::Bool) => {
            optimization_mut(settings).failure_classification = Some(parse_bool(spec.key, value)?)
        }
        ("optimization.self_healing", ValueKind::Bool) => {
            optimization_mut(settings).self_healing = Some(parse_bool(spec.key, value)?)
        }
        ("optimization.self_healing_max_retries", ValueKind::Usize) => {
            optimization_mut(settings).self_healing_max_retries =
                Some(parse_usize(spec.key, value)?)
        }
        ("optimization.action_caching", ValueKind::Bool) => {
            optimization_mut(settings).action_caching = Some(parse_bool(spec.key, value)?)
        }
        ("optimization.action_cache_ttl_secs", ValueKind::U64) => {
            optimization_mut(settings).action_cache_ttl_secs = Some(parse_u64(spec.key, value)?)
        }
        ("optimization.confidence_tracking", ValueKind::Bool) => {
            optimization_mut(settings).confidence_tracking = Some(parse_bool(spec.key, value)?)
        }
        ("optimization.compound_enrichment", ValueKind::Bool) => {
            optimization_mut(settings).compound_enrichment = Some(parse_bool(spec.key, value)?)
        }
        ("optimization.budget_max_session_cost_usd", ValueKind::NullableFloat) => {
            optimization_mut(settings).budget_max_session_cost_usd =
                parse_nullable_float(spec.key, value)?
        }
        ("optimization.budget_enforcement", ValueKind::NullableEnum(allowed)) => {
            optimization_mut(settings).budget_enforcement =
                parse_nullable_enum(spec.key, value, allowed)?
        }
        ("optimization.budget_warn_threshold_pct", ValueKind::U32) => {
            optimization_mut(settings).budget_warn_threshold_pct = Some(parse_u32(spec.key, value)?)
        }
        ("optimization.per_agent_cost_tracking", ValueKind::Bool) => {
            optimization_mut(settings).per_agent_cost_tracking = Some(parse_bool(spec.key, value)?)
        }
        ("optimization.content_aware_profiles", ValueKind::Bool) => {
            optimization_mut(settings).content_aware_profiles = Some(parse_bool(spec.key, value)?)
        }
        ("script.max_steps", ValueKind::Usize) => {
            script_mut(settings).max_steps = Some(parse_usize(spec.key, value)?)
        }
        ("script.max_timeout_secs", ValueKind::U64) => {
            script_mut(settings).max_timeout_secs = Some(parse_u64(spec.key, value)?)
        }
        ("script.max_output_bytes", ValueKind::Usize) => {
            script_mut(settings).max_output_bytes = Some(parse_usize(spec.key, value)?)
        }
        ("script.max_parallel_branches", ValueKind::Usize) => {
            script_mut(settings).max_parallel_branches = Some(parse_usize(spec.key, value)?)
        }
        ("script.max_concurrent_scripts", ValueKind::Usize) => {
            script_mut(settings).max_concurrent_scripts = Some(parse_usize(spec.key, value)?)
        }
        ("script.per_step_timeout_secs", ValueKind::U64) => {
            script_mut(settings).per_step_timeout_secs = Some(parse_u64(spec.key, value)?)
        }
        ("script.max_script_size_bytes", ValueKind::Usize) => {
            script_mut(settings).max_script_size_bytes = Some(parse_usize(spec.key, value)?)
        }
        ("script.max_nesting_depth", ValueKind::Usize) => {
            script_mut(settings).max_nesting_depth = Some(parse_usize(spec.key, value)?)
        }
        ("script.scripts_dir", ValueKind::Path | ValueKind::String) => {
            script_mut(settings).scripts_dir = Some(PathBuf::from(value))
        }
        _ => return Err(bad_value(spec.key, value, "unsupported schema mapping")),
    }
    Ok(())
}

fn apply_unset(settings: &mut Settings, key: &str) {
    match key {
        "headless" => settings.headless = None,
        "max_steps" => settings.max_steps = None,
        "model" => settings.model = None,
        "reasoning_effort" => settings.reasoning_effort = None,
        "output_dir" => settings.output_dir = None,
        "auto_compact_input_tokens" => settings.auto_compact_input_tokens = None,
        "max_concurrent_per_parent" => settings.max_concurrent_per_parent = None,
        "max_fork_depth" => settings.max_fork_depth = None,
        "max_total_agents" => settings.max_total_agents = None,
        "fork_child_max_steps" => settings.fork_child_max_steps = None,
        "fork_wait_timeout_secs" => settings.fork_wait_timeout_secs = None,
        "extension_bridge_token" => settings.extension_bridge_token = None,
        "extension_bridge_port" => settings.extension_bridge_port = None,
        "browser_backend" => settings.browser_backend = None,
        "compaction_prune_protect_tokens" => settings.compaction_prune_protect_tokens = None,
        "compaction_prune_max_output_chars" => settings.compaction_prune_max_output_chars = None,
        "compaction_preserve_recent_tokens" => settings.compaction_preserve_recent_tokens = None,
        "compaction_preserve_recent_messages_floor" => {
            settings.compaction_preserve_recent_messages_floor = None
        }
        "compaction_max_summary_chars" => settings.compaction_max_summary_chars = None,
        "compaction_llm_summarization" => settings.compaction_llm_summarization = None,
        "optimization.html_diff_mode" => {
            set_optimization_field(settings, |o| o.html_diff_mode = None)
        }
        "optimization.loop_detection" => {
            set_optimization_field(settings, |o| o.loop_detection = None)
        }
        "optimization.loop_detection_window" => {
            set_optimization_field(settings, |o| o.loop_detection_window = None)
        }
        "optimization.loop_nudge_threshold" => {
            set_optimization_field(settings, |o| o.loop_nudge_threshold = None)
        }
        "optimization.page_fingerprinting" => {
            set_optimization_field(settings, |o| o.page_fingerprinting = None)
        }
        "optimization.planning_interval" => {
            set_optimization_field(settings, |o| o.planning_interval = None)
        }
        "optimization.failure_classification" => {
            set_optimization_field(settings, |o| o.failure_classification = None)
        }
        "optimization.self_healing" => set_optimization_field(settings, |o| o.self_healing = None),
        "optimization.self_healing_max_retries" => {
            set_optimization_field(settings, |o| o.self_healing_max_retries = None)
        }
        "optimization.action_caching" => {
            set_optimization_field(settings, |o| o.action_caching = None)
        }
        "optimization.action_cache_ttl_secs" => {
            set_optimization_field(settings, |o| o.action_cache_ttl_secs = None)
        }
        "optimization.confidence_tracking" => {
            set_optimization_field(settings, |o| o.confidence_tracking = None)
        }
        "optimization.compound_enrichment" => {
            set_optimization_field(settings, |o| o.compound_enrichment = None)
        }
        "optimization.budget_max_session_cost_usd" => {
            set_optimization_field(settings, |o| o.budget_max_session_cost_usd = None)
        }
        "optimization.budget_enforcement" => {
            set_optimization_field(settings, |o| o.budget_enforcement = None)
        }
        "optimization.budget_warn_threshold_pct" => {
            set_optimization_field(settings, |o| o.budget_warn_threshold_pct = None)
        }
        "optimization.per_agent_cost_tracking" => {
            set_optimization_field(settings, |o| o.per_agent_cost_tracking = None)
        }
        "optimization.content_aware_profiles" => {
            set_optimization_field(settings, |o| o.content_aware_profiles = None)
        }
        "script.max_steps" => set_script_field(settings, |s| s.max_steps = None),
        "script.max_timeout_secs" => set_script_field(settings, |s| s.max_timeout_secs = None),
        "script.max_output_bytes" => set_script_field(settings, |s| s.max_output_bytes = None),
        "script.max_parallel_branches" => {
            set_script_field(settings, |s| s.max_parallel_branches = None)
        }
        "script.max_concurrent_scripts" => {
            set_script_field(settings, |s| s.max_concurrent_scripts = None)
        }
        "script.per_step_timeout_secs" => {
            set_script_field(settings, |s| s.per_step_timeout_secs = None)
        }
        "script.max_script_size_bytes" => {
            set_script_field(settings, |s| s.max_script_size_bytes = None)
        }
        "script.max_nesting_depth" => set_script_field(settings, |s| s.max_nesting_depth = None),
        "script.scripts_dir" => set_script_field(settings, |s| s.scripts_dir = None),
        _ => {}
    }
}

fn optimization_mut(settings: &mut Settings) -> &mut OptimizationSettings {
    settings
        .optimization
        .get_or_insert_with(empty_optimization_settings)
}

fn script_mut(settings: &mut Settings) -> &mut ScriptSettings {
    settings.script.get_or_insert_with(empty_script_settings)
}

fn set_optimization_field(settings: &mut Settings, mutate: impl FnOnce(&mut OptimizationSettings)) {
    if let Some(optimization) = settings.optimization.as_mut() {
        mutate(optimization);
    }
}

fn set_script_field(settings: &mut Settings, mutate: impl FnOnce(&mut ScriptSettings)) {
    if let Some(script) = settings.script.as_mut() {
        mutate(script);
    }
}

fn empty_optimization_settings() -> OptimizationSettings {
    OptimizationSettings::default()
}

fn empty_script_settings() -> ScriptSettings {
    ScriptSettings {
        max_steps: None,
        max_timeout_secs: None,
        max_output_bytes: None,
        max_parallel_branches: None,
        max_concurrent_scripts: None,
        per_step_timeout_secs: None,
        max_script_size_bytes: None,
        max_nesting_depth: None,
        scripts_dir: None,
    }
}

fn prune_empty_blocks(settings: &mut Settings) {
    if settings
        .optimization
        .as_ref()
        .is_some_and(optimization_is_empty)
    {
        settings.optimization = None;
    }
    if settings.script.as_ref().is_some_and(script_is_empty) {
        settings.script = None;
    }
}

fn optimization_is_empty(optimization: &OptimizationSettings) -> bool {
    optimization.html_diff_mode.is_none()
        && optimization.loop_detection.is_none()
        && optimization.loop_detection_window.is_none()
        && optimization.loop_nudge_threshold.is_none()
        && optimization.page_fingerprinting.is_none()
        && optimization.planning_interval.is_none()
        && optimization.failure_classification.is_none()
        && optimization.self_healing.is_none()
        && optimization.self_healing_max_retries.is_none()
        && optimization.action_caching.is_none()
        && optimization.action_cache_ttl_secs.is_none()
        && optimization.confidence_tracking.is_none()
        && optimization.compound_enrichment.is_none()
        && optimization.content_aware_profiles.is_none()
        && optimization.budget_max_session_cost_usd.is_none()
        && optimization.budget_enforcement.is_none()
        && optimization.budget_warn_threshold_pct.is_none()
        && optimization.per_agent_cost_tracking.is_none()
}

fn script_is_empty(script: &ScriptSettings) -> bool {
    script.max_steps.is_none()
        && script.max_timeout_secs.is_none()
        && script.max_output_bytes.is_none()
        && script.max_parallel_branches.is_none()
        && script.max_concurrent_scripts.is_none()
        && script.per_step_timeout_secs.is_none()
        && script.max_script_size_bytes.is_none()
        && script.max_nesting_depth.is_none()
        && script.scripts_dir.is_none()
}

fn stored_value(settings: &Settings, key: &str) -> Value {
    match key {
        "headless" => json!(settings.headless),
        "max_steps" => json!(settings.max_steps),
        "model" => json!(settings.model),
        "reasoning_effort" => json!(settings.reasoning_effort),
        "output_dir" => json!(settings.output_dir),
        "auto_compact_input_tokens" => json!(settings.auto_compact_input_tokens),
        "max_concurrent_per_parent" => json!(settings.max_concurrent_per_parent),
        "max_fork_depth" => json!(settings.max_fork_depth),
        "max_total_agents" => json!(settings.max_total_agents),
        "fork_child_max_steps" => json!(settings.fork_child_max_steps),
        "fork_wait_timeout_secs" => json!(settings.fork_wait_timeout_secs),
        "extension_bridge_token" => json!(settings.extension_bridge_token),
        "extension_bridge_port" => json!(settings.extension_bridge_port),
        "browser_backend" => json!(settings.browser_backend),
        "compaction_prune_protect_tokens" => json!(settings.compaction_prune_protect_tokens),
        "compaction_prune_max_output_chars" => json!(settings.compaction_prune_max_output_chars),
        "compaction_preserve_recent_tokens" => json!(settings.compaction_preserve_recent_tokens),
        "compaction_preserve_recent_messages_floor" => {
            json!(settings.compaction_preserve_recent_messages_floor)
        }
        "compaction_max_summary_chars" => json!(settings.compaction_max_summary_chars),
        "compaction_llm_summarization" => json!(settings.compaction_llm_summarization),
        "optimization.html_diff_mode" => {
            json!(settings
                .optimization
                .as_ref()
                .and_then(|o| o.html_diff_mode))
        }
        "optimization.loop_detection" => {
            json!(settings
                .optimization
                .as_ref()
                .and_then(|o| o.loop_detection))
        }
        "optimization.loop_detection_window" => {
            json!(settings
                .optimization
                .as_ref()
                .and_then(|o| o.loop_detection_window))
        }
        "optimization.loop_nudge_threshold" => {
            json!(settings
                .optimization
                .as_ref()
                .and_then(|o| o.loop_nudge_threshold))
        }
        "optimization.page_fingerprinting" => {
            json!(settings
                .optimization
                .as_ref()
                .and_then(|o| o.page_fingerprinting))
        }
        "optimization.planning_interval" => {
            json!(settings
                .optimization
                .as_ref()
                .and_then(|o| o.planning_interval))
        }
        "optimization.failure_classification" => {
            json!(settings
                .optimization
                .as_ref()
                .and_then(|o| o.failure_classification))
        }
        "optimization.self_healing" => {
            json!(settings.optimization.as_ref().and_then(|o| o.self_healing))
        }
        "optimization.self_healing_max_retries" => {
            json!(settings
                .optimization
                .as_ref()
                .and_then(|o| o.self_healing_max_retries))
        }
        "optimization.action_caching" => {
            json!(settings
                .optimization
                .as_ref()
                .and_then(|o| o.action_caching))
        }
        "optimization.action_cache_ttl_secs" => {
            json!(settings
                .optimization
                .as_ref()
                .and_then(|o| o.action_cache_ttl_secs))
        }
        "optimization.confidence_tracking" => {
            json!(settings
                .optimization
                .as_ref()
                .and_then(|o| o.confidence_tracking))
        }
        "optimization.compound_enrichment" => {
            json!(settings
                .optimization
                .as_ref()
                .and_then(|o| o.compound_enrichment))
        }
        "optimization.budget_max_session_cost_usd" => {
            json!(settings
                .optimization
                .as_ref()
                .and_then(|o| o.budget_max_session_cost_usd))
        }
        "optimization.budget_enforcement" => {
            json!(settings
                .optimization
                .as_ref()
                .and_then(|o| o.budget_enforcement.clone()))
        }
        "optimization.budget_warn_threshold_pct" => {
            json!(settings
                .optimization
                .as_ref()
                .and_then(|o| o.budget_warn_threshold_pct))
        }
        "optimization.per_agent_cost_tracking" => {
            json!(settings
                .optimization
                .as_ref()
                .and_then(|o| o.per_agent_cost_tracking))
        }
        "optimization.content_aware_profiles" => {
            json!(settings
                .optimization
                .as_ref()
                .and_then(|o| o.content_aware_profiles))
        }
        "script.max_steps" => json!(settings.script.as_ref().and_then(|s| s.max_steps)),
        "script.max_timeout_secs" => {
            json!(settings.script.as_ref().and_then(|s| s.max_timeout_secs))
        }
        "script.max_output_bytes" => {
            json!(settings.script.as_ref().and_then(|s| s.max_output_bytes))
        }
        "script.max_parallel_branches" => {
            json!(settings
                .script
                .as_ref()
                .and_then(|s| s.max_parallel_branches))
        }
        "script.max_concurrent_scripts" => {
            json!(settings
                .script
                .as_ref()
                .and_then(|s| s.max_concurrent_scripts))
        }
        "script.per_step_timeout_secs" => {
            json!(settings
                .script
                .as_ref()
                .and_then(|s| s.per_step_timeout_secs))
        }
        "script.max_script_size_bytes" => {
            json!(settings
                .script
                .as_ref()
                .and_then(|s| s.max_script_size_bytes))
        }
        "script.max_nesting_depth" => {
            json!(settings.script.as_ref().and_then(|s| s.max_nesting_depth))
        }
        "script.scripts_dir" => json!(settings.script.as_ref().and_then(|s| s.scripts_dir.clone())),
        _ => Value::Null,
    }
}

fn effective_value(settings: &Settings, key: &str) -> Value {
    match key {
        "headless" => json!(settings_get_headless(settings)),
        "max_steps" => json!(settings_get_max_steps(settings)),
        "model" => json!(settings.model.clone()),
        "reasoning_effort" => json!(settings
            .reasoning_effort
            .clone()
            .unwrap_or_else(|| "high".to_string())),
        "output_dir" => json!(settings_get_output_dir(settings)),
        "auto_compact_input_tokens" => json!(settings_get_auto_compact_tokens(settings)),
        "max_concurrent_per_parent" => json!(settings_get_max_concurrent_per_parent(settings)),
        "max_fork_depth" => json!(settings_get_max_fork_depth(settings)),
        "max_total_agents" => json!(settings_get_max_total_agents(settings)),
        "fork_child_max_steps" => json!(settings_get_fork_child_max_steps(settings)),
        "fork_wait_timeout_secs" => json!(settings_get_fork_wait_timeout_secs(settings)),
        "extension_bridge_token" => json!(settings.extension_bridge_token.clone()),
        "extension_bridge_port" => json!(settings.extension_bridge_port.unwrap_or(19_876)),
        "browser_backend" => json!(settings.browser_backend.clone()),
        "compaction_prune_protect_tokens" => {
            json!(settings_get_compaction_prune_protect_tokens(settings))
        }
        "compaction_prune_max_output_chars" => {
            json!(settings_get_compaction_prune_max_output_chars(settings))
        }
        "compaction_preserve_recent_tokens" => {
            json!(settings_get_compaction_preserve_recent_tokens(settings))
        }
        "compaction_preserve_recent_messages_floor" => {
            json!(settings_get_compaction_preserve_recent_messages_floor(
                settings
            ))
        }
        "compaction_max_summary_chars" => {
            json!(settings_get_compaction_max_summary_chars(settings))
        }
        "compaction_llm_summarization" => {
            json!(settings_get_compaction_llm_summarization(settings))
        }
        "optimization.html_diff_mode" => json!(settings_get_html_diff_mode(settings)),
        "optimization.loop_detection" => json!(settings_get_loop_detection(settings)),
        "optimization.loop_detection_window" => json!(settings_get_loop_detection_window(settings)),
        "optimization.loop_nudge_threshold" => json!(settings_get_loop_nudge_threshold(settings)),
        "optimization.page_fingerprinting" => json!(settings_get_page_fingerprinting(settings)),
        "optimization.planning_interval" => json!(settings_get_planning_interval(settings)),
        "optimization.failure_classification" => {
            json!(settings_get_failure_classification(settings))
        }
        "optimization.self_healing" => json!(settings_get_self_healing(settings)),
        "optimization.self_healing_max_retries" => {
            json!(settings_get_self_healing_max_retries(settings))
        }
        "optimization.action_caching" => json!(settings_get_action_caching(settings)),
        "optimization.action_cache_ttl_secs" => json!(settings_get_action_cache_ttl_secs(settings)),
        "optimization.confidence_tracking" => json!(settings_get_confidence_tracking(settings)),
        "optimization.compound_enrichment" => json!(settings_get_compound_enrichment(settings)),
        "optimization.budget_max_session_cost_usd" => {
            json!(settings_get_budget_max_session_cost_usd(settings))
        }
        "optimization.budget_enforcement" => json!(settings_get_budget_enforcement(settings)),
        "optimization.budget_warn_threshold_pct" => {
            json!(settings_get_budget_warn_threshold_pct(settings))
        }
        "optimization.per_agent_cost_tracking" => {
            json!(settings_get_per_agent_cost_tracking(settings))
        }
        "optimization.content_aware_profiles" => {
            json!(settings_get_content_aware_profiles(settings))
        }
        "script.max_steps" => json!(effective_script_settings(settings).max_steps),
        "script.max_timeout_secs" => json!(effective_script_settings(settings).max_timeout_secs),
        "script.max_output_bytes" => json!(effective_script_settings(settings).max_output_bytes),
        "script.max_parallel_branches" => {
            json!(effective_script_settings(settings).max_parallel_branches)
        }
        "script.max_concurrent_scripts" => {
            json!(effective_script_settings(settings).max_concurrent_scripts)
        }
        "script.per_step_timeout_secs" => {
            json!(effective_script_settings(settings).per_step_timeout_secs)
        }
        "script.max_script_size_bytes" => {
            json!(effective_script_settings(settings).max_script_size_bytes)
        }
        "script.max_nesting_depth" => json!(effective_script_settings(settings).max_nesting_depth),
        "script.scripts_dir" => json!(effective_script_settings(settings).scripts_dir),
        _ => Value::Null,
    }
}

fn effective_settings(settings: &Settings) -> Settings {
    Settings {
        headless: Some(settings_get_headless(settings)),
        max_steps: Some(settings_get_max_steps(settings)),
        model: settings.model.clone(),
        reasoning_effort: Some(
            settings
                .reasoning_effort
                .clone()
                .unwrap_or_else(|| "high".to_string()),
        ),
        output_dir: Some(settings_get_output_dir(settings).to_string()),
        auto_compact_input_tokens: Some(settings_get_auto_compact_tokens(settings)),
        max_concurrent_per_parent: Some(settings_get_max_concurrent_per_parent(settings)),
        max_fork_depth: Some(settings_get_max_fork_depth(settings)),
        max_total_agents: Some(settings_get_max_total_agents(settings)),
        fork_child_max_steps: Some(settings_get_fork_child_max_steps(settings)),
        fork_wait_timeout_secs: Some(settings_get_fork_wait_timeout_secs(settings)),
        extension_bridge_token: settings.extension_bridge_token.clone(),
        extension_bridge_port: Some(settings.extension_bridge_port.unwrap_or(19_876)),
        browser_backend: settings.browser_backend.clone(),
        compaction_prune_protect_tokens: Some(
            settings_get_compaction_prune_protect_tokens(settings) as u64,
        ),
        compaction_prune_max_output_chars: Some(settings_get_compaction_prune_max_output_chars(
            settings,
        ) as u64),
        compaction_preserve_recent_tokens: Some(settings_get_compaction_preserve_recent_tokens(
            settings,
        ) as u64),
        compaction_preserve_recent_messages_floor: Some(
            settings_get_compaction_preserve_recent_messages_floor(settings) as u32,
        ),
        compaction_max_summary_chars: Some(
            settings_get_compaction_max_summary_chars(settings) as u64
        ),
        compaction_llm_summarization: Some(settings_get_compaction_llm_summarization(settings)),
        script: Some(effective_script_settings(settings)),
        optimization: Some(OptimizationSettings {
            html_diff_mode: Some(settings_get_html_diff_mode(settings)),
            loop_detection: Some(settings_get_loop_detection(settings)),
            loop_detection_window: Some(settings_get_loop_detection_window(settings)),
            loop_nudge_threshold: Some(settings_get_loop_nudge_threshold(settings)),
            page_fingerprinting: Some(settings_get_page_fingerprinting(settings)),
            planning_interval: Some(settings_get_planning_interval(settings)),
            failure_classification: Some(settings_get_failure_classification(settings)),
            self_healing: Some(settings_get_self_healing(settings)),
            self_healing_max_retries: Some(settings_get_self_healing_max_retries(settings)),
            action_caching: Some(settings_get_action_caching(settings)),
            action_cache_ttl_secs: Some(settings_get_action_cache_ttl_secs(settings)),
            confidence_tracking: Some(settings_get_confidence_tracking(settings)),
            compound_enrichment: Some(settings_get_compound_enrichment(settings)),
            content_aware_profiles: Some(settings_get_content_aware_profiles(settings)),
            budget_max_session_cost_usd: settings_get_budget_max_session_cost_usd(settings),
            budget_enforcement: settings_get_budget_enforcement(settings),
            budget_warn_threshold_pct: Some(settings_get_budget_warn_threshold_pct(settings)),
            per_agent_cost_tracking: Some(settings_get_per_agent_cost_tracking(settings)),
        }),
    }
}

fn effective_script_settings(settings: &Settings) -> ScriptSettings {
    let mut effective = ScriptSettings::default();
    if let Some(script) = settings.script.as_ref() {
        if let Some(value) = script.max_steps {
            effective.max_steps = Some(value);
        }
        if let Some(value) = script.max_timeout_secs {
            effective.max_timeout_secs = Some(value);
        }
        if let Some(value) = script.max_output_bytes {
            effective.max_output_bytes = Some(value);
        }
        if let Some(value) = script.max_parallel_branches {
            effective.max_parallel_branches = Some(value);
        }
        if let Some(value) = script.max_concurrent_scripts {
            effective.max_concurrent_scripts = Some(value);
        }
        if let Some(value) = script.per_step_timeout_secs {
            effective.per_step_timeout_secs = Some(value);
        }
        if let Some(value) = script.max_script_size_bytes {
            effective.max_script_size_bytes = Some(value);
        }
        if let Some(value) = script.max_nesting_depth {
            effective.max_nesting_depth = Some(value);
        }
        if let Some(value) = script.scripts_dir.clone() {
            effective.scripts_dir = Some(value);
        }
    }
    effective
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn test_env_lock() -> std::sync::MutexGuard<'static, ()> {
        crate::test_env_lock()
    }

    fn unique_temp_dir() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!(
            "acrawl_config_ops_test_{}_{}",
            std::process::id(),
            nanos
        ))
    }

    fn setup_temp_dir() -> PathBuf {
        let temp_dir = unique_temp_dir();
        fs::create_dir_all(&temp_dir).expect("create temp dir");
        temp_dir
    }

    fn cleanup_temp_dir(path: &Path) {
        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn config_round_trip_and_get_dump_work() {
        let _lock = test_env_lock();
        let temp_dir = setup_temp_dir();
        std::env::set_var("ACRAWL_CONFIG_HOME", &temp_dir);

        config_set("headless", "false").expect("set headless");
        config_set("max_steps", "77").expect("set max_steps");
        config_set("optimization.loop_detection", "true").expect("set nested bool");
        config_set("script.max_timeout_secs", "999").expect("set nested uint");

        assert_eq!(
            config_get("headless", false).expect("get headless"),
            "false"
        );
        assert_eq!(config_get("max_steps", false).expect("get max_steps"), "77");
        assert_eq!(
            config_get("optimization.loop_detection", false).expect("get loop_detection"),
            "true"
        );
        assert_eq!(
            config_get("script.max_timeout_secs", false).expect("get script timeout"),
            "999"
        );

        let dumped = config_get("", false).expect("dump settings");
        let parsed: Value = serde_json::from_str(&dumped).expect("parse dumped json");
        assert_eq!(parsed["headless"], Value::Bool(false));
        assert_eq!(parsed["max_steps"], Value::from(77));
        assert_eq!(parsed["optimization"]["loop_detection"], Value::Bool(true));
        assert_eq!(parsed["script"]["max_timeout_secs"], Value::from(999));

        cleanup_temp_dir(&temp_dir);
    }

    #[test]
    fn config_set_unknown_key_returns_error() {
        let _lock = test_env_lock();
        let temp_dir = setup_temp_dir();
        std::env::set_var("ACRAWL_CONFIG_HOME", &temp_dir);

        let err = config_set("nope", "true").expect_err("unknown key should fail");
        assert!(matches!(err, ConfigError::UnknownKey(key) if key == "nope"));

        cleanup_temp_dir(&temp_dir);
    }

    #[test]
    fn config_set_bad_type_returns_error() {
        let _lock = test_env_lock();
        let temp_dir = setup_temp_dir();
        std::env::set_var("ACRAWL_CONFIG_HOME", &temp_dir);

        let err = config_set("max_steps", "abc").expect_err("bad type should fail");
        assert!(matches!(
            err,
            ConfigError::BadValue { key, value, .. } if key == "max_steps" && value == "abc"
        ));

        cleanup_temp_dir(&temp_dir);
    }

    #[test]
    fn output_dir_stays_literal_string() {
        let _lock = test_env_lock();
        let temp_dir = setup_temp_dir();
        std::env::set_var("ACRAWL_CONFIG_HOME", &temp_dir);

        config_set("output_dir", "true").expect("set output dir");
        assert_eq!(
            config_get("output_dir", false).expect("get output dir"),
            "\"true\""
        );

        let settings = load_settings();
        assert_eq!(settings.output_dir.as_deref(), Some("true"));

        cleanup_temp_dir(&temp_dir);
    }

    #[test]
    fn dot_path_nesting_and_block_unset_work() {
        let _lock = test_env_lock();
        let temp_dir = setup_temp_dir();
        std::env::set_var("ACRAWL_CONFIG_HOME", &temp_dir);

        config_set("optimization.self_healing", "true").expect("set nested field");
        config_set("script.max_steps", "321").expect("set nested script field");

        let settings = load_settings();
        assert_eq!(
            settings
                .optimization
                .as_ref()
                .and_then(|optimization| optimization.self_healing),
            Some(true)
        );
        assert_eq!(
            settings.script.as_ref().and_then(|script| script.max_steps),
            Some(321)
        );

        config_unset("optimization").expect("unset optimization block");
        config_unset("script").expect("unset script block");

        let settings = load_settings();
        assert!(settings.optimization.is_none());
        assert!(settings.script.is_none());

        cleanup_temp_dir(&temp_dir);
    }

    #[test]
    fn effective_fallback_uses_getters() {
        let _lock = test_env_lock();
        let temp_dir = setup_temp_dir();
        std::env::set_var("ACRAWL_CONFIG_HOME", &temp_dir);

        config_set("max_steps", "88").expect("set max_steps");
        config_unset("max_steps").expect("unset max_steps");

        assert_eq!(config_get("max_steps", false).expect("raw get"), "null");
        assert_eq!(config_get("max_steps", true).expect("effective get"), "50");

        cleanup_temp_dir(&temp_dir);
    }

    #[test]
    fn model_requires_provider_prefix_slash() {
        let _lock = test_env_lock();
        let temp_dir = setup_temp_dir();
        std::env::set_var("ACRAWL_CONFIG_HOME", &temp_dir);

        let err = config_set("model", "gpt-5").expect_err("model without slash should fail");
        assert!(matches!(
            err,
            ConfigError::BadValue { key, value, .. } if key == "model" && value == "gpt-5"
        ));

        cleanup_temp_dir(&temp_dir);
    }
}
