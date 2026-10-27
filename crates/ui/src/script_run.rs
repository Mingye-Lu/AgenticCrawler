//! `acrawl run`: execute a saved or file-based script without an LLM.
//!
//! The script executor only needs a browser, so a saved script can run from
//! cron, CI, or any other process without waking an agent or reading
//! provider credentials. The script is validated before the browser launches,
//! so a typo fails in milliseconds with exit code 2.

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use acrawl_core::{ScriptLimits, ScriptResult, ScriptStatus, ScriptTask};
use agent::extension_bridge::{connect_timeout, extension_backend_selected};
use agent::script_manager::{ScriptError, ScriptManager};
use agent::ExtensionBridgeManager;
use browser::{BrowserBackend, BrowserContext, PlaywrightBridge, SharedBridge};
use script::parser::{parse_script, validate_script};
use script::persistence::load_script_from_disk;
use serde_json::Value;

use crate::TOKIO_RUNTIME;

const STATUS_POLL_INTERVAL: Duration = Duration::from_millis(250);

pub const EXIT_OK: i32 = 0;
pub const EXIT_FAILED: i32 = 1;
pub const EXIT_USAGE: i32 = 2;
pub const EXIT_INTERRUPTED: i32 = 130;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunFormat {
    /// `extracted_data` as one JSON array.
    Json,
    /// One `extracted_data` item per line.
    Jsonl,
    /// The full `ScriptResult` (status, error, yielded data, timings).
    Result,
}

impl RunFormat {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "json" => Ok(Self::Json),
            "jsonl" => Ok(Self::Jsonl),
            "result" => Ok(Self::Result),
            other => Err(format!(
                "unsupported value for --format: {other} (expected json, jsonl, or result)"
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOptions {
    /// A saved script name, or a path to a script JSON file.
    pub script: String,
    pub format: RunFormat,
    pub output: Option<PathBuf>,
    pub flatten: bool,
    pub quiet: bool,
    pub max_steps: Option<usize>,
    pub timeout_secs: Option<u64>,
    pub step_timeout_secs: Option<u64>,
}

/// Runs a script to completion and returns the process exit code.
#[must_use]
pub fn run_script_cli(options: &RunOptions) -> i32 {
    let settings = runtime::load_settings().script.unwrap_or_default();
    let scripts_dir = settings.resolved_scripts_dir();
    let manager = ScriptManager::new(settings);
    // Validate against the same settings-capped limits `spawn_script` uses,
    // so a script over a configured cap fails here, not after browser launch.
    let limits = manager.effective_limits(&requested_limits(options));

    let script = match resolve_script(&options.script, &scripts_dir)
        .and_then(|script| prevalidate(&script, &limits).map(|()| script))
    {
        Ok(script) => script,
        Err(error) => {
            eprintln!("error: {error}");
            return EXIT_USAGE;
        }
    };

    let rt = TOKIO_RUNTIME
        .get()
        .expect("tokio runtime must be initialized before run_script_cli");
    rt.block_on(run_resolved(script, limits, manager, options))
}

async fn run_resolved(
    script: Value,
    limits: ScriptLimits,
    mut manager: ScriptManager,
    options: &RunOptions,
) -> i32 {
    // Held until the script finishes: dropping the manager stops the bridge
    // server, and dropping the CloakBrowser bridge kills its subprocess.
    let mut bridge_manager = None;
    let shared = match launch_browser(&mut bridge_manager).await {
        Ok(shared) => shared,
        Err(error) => {
            eprintln!("error: failed to launch browser: {error}");
            return EXIT_FAILED;
        }
    };

    let task = ScriptTask {
        script,
        save_as: None,
        limits,
    };
    let script_id = match manager.spawn_script(task, BrowserContext::new(shared)) {
        Ok(id) => id,
        Err(ScriptError::ParseError(message)) => {
            eprintln!("error: script parse/validation failed: {message}");
            return EXIT_USAGE;
        }
        Err(error) => {
            eprintln!("error: {error}");
            return EXIT_FAILED;
        }
    };

    let interrupted = watch_until_finished(&manager, &script_id, options.quiet).await;
    let result = match manager.wait_for_scripts(Some(vec![script_id])).await {
        Ok(mut results) if !results.is_empty() => results.remove(0),
        Ok(_) => {
            eprintln!("error: script produced no result");
            return EXIT_FAILED;
        }
        Err(error) => {
            eprintln!("error: {error}");
            return EXIT_FAILED;
        }
    };
    drop(manager);
    drop(bridge_manager);

    let items = match emit(&result, options) {
        Ok(items) => items,
        Err(error) => {
            eprintln!("error: failed to write output: {error}");
            return EXIT_FAILED;
        }
    };
    if !options.quiet {
        eprintln!("{}", summary_line(&result, items));
    }

    if interrupted {
        EXIT_INTERRUPTED
    } else {
        exit_code_for(&result)
    }
}

fn requested_limits(options: &RunOptions) -> ScriptLimits {
    let mut limits = ScriptLimits::default();
    if let Some(max_steps) = options.max_steps {
        limits.max_steps = max_steps;
    }
    if let Some(secs) = options.timeout_secs {
        limits.max_timeout_secs = secs;
    }
    if let Some(secs) = options.step_timeout_secs {
        limits.per_step_timeout_secs = secs;
    }
    limits
}

/// A path-like argument (`*.json`, or containing a separator) is read from
/// disk; anything else is a name saved under the scripts directory.
fn resolve_script(source: &str, scripts_dir: &Path) -> Result<Value, String> {
    if looks_like_path(source) {
        let text = std::fs::read_to_string(source)
            .map_err(|error| format!("failed to read script file `{source}`: {error}"))?;
        return serde_json::from_str(&text)
            .map_err(|error| format!("script file `{source}` is not valid JSON: {error}"));
    }
    let definition = load_script_from_disk(scripts_dir, source).map_err(|error| {
        format!(
            "failed to load saved script `{source}` from {}: {error}",
            scripts_dir.display()
        )
    })?;
    serde_json::to_value(&definition)
        .map_err(|error| format!("failed to re-encode script `{source}`: {error}"))
}

fn looks_like_path(source: &str) -> bool {
    Path::new(source)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
        || source.contains('/')
        || source.contains('\\')
}

fn prevalidate(script: &Value, limits: &ScriptLimits) -> Result<(), String> {
    let definition = parse_script(script).map_err(|error| format!("invalid script: {error}"))?;
    validate_script(&definition, limits).map_err(|errors| {
        let message = errors
            .into_iter()
            .map(|error| error.to_string())
            .collect::<Vec<_>>()
            .join("; ");
        format!("script validation failed: {message}")
    })
}

/// Extension mode drives the user's own logged-in browser, so it never falls
/// back to `CloakBrowser`: an unreachable extension is an error.
async fn launch_browser(
    bridge_manager: &mut Option<ExtensionBridgeManager>,
) -> Result<SharedBridge, String> {
    if !extension_backend_selected() {
        let bridge = PlaywrightBridge::new()
            .await
            .map_err(|error| error.to_string())?;
        return Ok(Arc::new(tokio::sync::Mutex::new(
            Box::new(bridge) as Box<dyn BrowserBackend + Send>
        )));
    }

    let manager = bridge_manager.insert(ExtensionBridgeManager::start_from_settings().await?);
    let timeout = connect_timeout();
    if manager.is_connected() || manager.wait_for_connection(timeout).await {
        return Ok(manager.shared_bridge());
    }
    Err(not_connected_error(timeout, manager.port()))
}

fn not_connected_error(timeout: Duration, port: u16) -> String {
    format!(
        "extension mode is enabled but the acrawl Bridge extension did not connect within \
         {secs}s on port {port}. Make sure the browser with the extension installed is open \
         and the extension's port matches, then retry. To use the bundled headless browser \
         instead, run `acrawl config unset browser_backend`.",
        secs = timeout.as_secs(),
    )
}

/// Streams yielded progress to stderr until the script ends. Returns true if
/// Ctrl+C cancelled it; the partial result is still collected and written.
async fn watch_until_finished(manager: &ScriptManager, script_id: &str, quiet: bool) -> bool {
    let mut printed = 0;
    let mut interrupted = false;
    let ctrl_c = tokio::signal::ctrl_c();
    tokio::pin!(ctrl_c);

    loop {
        if !quiet {
            if let Ok(state) = manager.get_status(script_id) {
                for item in state.yielded_data.iter().skip(printed) {
                    eprintln!("[yield] {}", compact_json(item));
                }
                printed = printed.max(state.yielded_data.len());
            }
        }

        let finished = manager
            .scripts
            .get(script_id)
            .is_none_or(|running| running.handle.is_finished());
        if finished {
            return interrupted;
        }

        tokio::select! {
            () = tokio::time::sleep(STATUS_POLL_INTERVAL) => {}
            _ = &mut ctrl_c, if !interrupted => {
                interrupted = true;
                eprintln!("Interrupted; cancelling script and keeping partial results...");
                let _ = manager.cancel_script(script_id);
            }
        }
    }
}

/// Writes the rendered result and returns how many data items it contained.
fn emit(result: &ScriptResult, options: &RunOptions) -> io::Result<usize> {
    let data = output_data(result, options.flatten);
    let rendered = render(result, &data, options.format)?;
    if let Some(path) = &options.output {
        write_atomically(path, rendered.as_bytes())?;
    } else {
        let mut stdout = io::stdout().lock();
        stdout.write_all(rendered.as_bytes())?;
        stdout.flush()?;
    }
    Ok(data.len())
}

fn output_data(result: &ScriptResult, flatten: bool) -> Vec<Value> {
    if flatten {
        flatten_one_level(&result.extracted_data)
    } else {
        result.extracted_data.clone()
    }
}

fn render(result: &ScriptResult, data: &[Value], format: RunFormat) -> io::Result<String> {
    let mut out = match format {
        RunFormat::Json => serde_json::to_string_pretty(data)?,
        RunFormat::Jsonl => data
            .iter()
            .map(serde_json::to_string)
            .collect::<Result<Vec<_>, _>>()?
            .join("\n"),
        RunFormat::Result => {
            let mut full = result.clone();
            full.extracted_data = data.to_vec();
            serde_json::to_string_pretty(&full)?
        }
    };
    if !out.is_empty() {
        out.push('\n');
    }
    Ok(out)
}

/// `collect` on a per-page array yields `[[a, b], [c]]`; flattening gives
/// downstream consumers one record per item.
fn flatten_one_level(items: &[Value]) -> Vec<Value> {
    items
        .iter()
        .flat_map(|item| match item {
            Value::Array(inner) => inner.clone(),
            other => vec![other.clone()],
        })
        .collect()
}

/// Writes to a sibling temp file and renames it over the target, so a reader
/// polling the output never sees a half-written file. Each writer creates its
/// own uniquely named temp file, so overlapping runs targeting the same path
/// cannot truncate or rename each other's in-progress output.
fn write_atomically(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    let (tmp, mut file) = create_unique_temp(path)?;
    let written = file
        .write_all(bytes)
        .and_then(|()| file.sync_all())
        .and_then(|()| {
            drop(file);
            std::fs::rename(&tmp, path)
        });
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written
}

/// `create_new` fails instead of reusing a file another writer owns, so a name
/// collision just moves on to the next candidate.
fn create_unique_temp(path: &Path) -> io::Result<(PathBuf, std::fs::File)> {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let pid = std::process::id();
    for attempt in 0..100 {
        let tmp = path.with_file_name(format!(".{name}.{pid}.{nanos}.{attempt}.tmp"));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
        {
            Ok(file) => return Ok((tmp, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        format!(
            "could not create a unique temp file next to {}",
            path.display()
        ),
    ))
}

fn summary_line(result: &ScriptResult, items: usize) -> String {
    let status = match result.status {
        ScriptStatus::Completed => "completed",
        ScriptStatus::Failed => "failed",
        ScriptStatus::Cancelled => "cancelled",
        ScriptStatus::Pending | ScriptStatus::Running => "unfinished",
    };
    let mut line = format!(
        "script {status}: {items} item(s), {} step(s), {:.1}s",
        result.steps_executed, result.elapsed_secs
    );
    if let Some(error) = &result.error {
        line.push_str("\nerror: ");
        line.push_str(error);
    }
    line
}

fn exit_code_for(result: &ScriptResult) -> i32 {
    match result.status {
        ScriptStatus::Completed => EXIT_OK,
        ScriptStatus::Cancelled => EXIT_INTERRUPTED,
        ScriptStatus::Failed | ScriptStatus::Pending | ScriptStatus::Running => EXIT_FAILED,
    }
}

fn compact_json(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn result_with(data: Vec<Value>, status: ScriptStatus) -> ScriptResult {
        ScriptResult {
            script_id: "scr_test".to_string(),
            status,
            extracted_data: data,
            yielded_data: vec![json!("page 1")],
            steps_executed: 2,
            elapsed_secs: 0.5,
            error: None,
        }
    }

    fn valid_script() -> Value {
        json!({
            "schema_version": 1,
            "name": "demo",
            "steps": [{"type": "collect", "value": {"kind": "literal", "value": "x"}}]
        })
    }

    #[test]
    fn path_detection_distinguishes_names_from_files() {
        assert!(looks_like_path("news.json"));
        assert!(looks_like_path("NEWS.JSON"));
        assert!(looks_like_path("./news"));
        assert!(looks_like_path(r"scripts\news"));
        assert!(!looks_like_path("mideast_headlines"));
        assert!(!looks_like_path("multi-search"));
    }

    #[test]
    fn resolves_script_from_file_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("demo.json");
        std::fs::write(&path, valid_script().to_string()).expect("write script");

        let script = resolve_script(path.to_str().expect("utf8 path"), dir.path())
            .expect("file should resolve");
        assert_eq!(script["name"], "demo");
    }

    #[test]
    fn resolves_saved_script_by_name() {
        let dir = tempfile::tempdir().expect("tempdir");
        let definition = parse_script(&valid_script()).expect("parse");
        script::persistence::save_script_to_disk(dir.path(), "demo", &definition)
            .expect("save script");

        let script = resolve_script("demo", dir.path()).expect("name should resolve");
        prevalidate(&script, &ScriptLimits::default()).expect("round-trip stays valid");
    }

    #[test]
    fn missing_saved_script_names_the_scripts_dir() {
        let dir = tempfile::tempdir().expect("tempdir");
        let error = resolve_script("nope", dir.path()).expect_err("missing script");
        assert!(error.contains("`nope`"), "{error}");
        assert!(error.contains(&dir.path().display().to_string()), "{error}");
    }

    #[test]
    fn prevalidate_rejects_undefined_variables_before_launch() {
        let script = json!({
            "schema_version": 1,
            "steps": [{"type": "collect", "value": {"kind": "variable", "value": "ghost"}}]
        });
        let error = prevalidate(&script, &ScriptLimits::default()).expect_err("invalid");
        assert!(error.contains("ghost"), "{error}");
    }

    #[test]
    fn settings_caps_apply_before_browser_launch() {
        let manager = ScriptManager::new(runtime::settings::ScriptSettings {
            max_script_size_bytes: Some(16),
            ..Default::default()
        });
        let limits = manager.effective_limits(&ScriptLimits::default());
        let error = prevalidate(&valid_script(), &limits).expect_err("over the cap");
        assert!(error.contains("exceeds maximum 16 bytes"), "{error}");
    }

    #[test]
    fn prevalidate_rejects_missing_schema_version() {
        let error =
            prevalidate(&json!({"steps": []}), &ScriptLimits::default()).expect_err("invalid");
        assert!(error.contains("schema_version"), "{error}");
    }

    #[test]
    fn json_format_renders_data_array() {
        let result = result_with(vec![json!({"a": 1}), json!("b")], ScriptStatus::Completed);
        let out = render(&result, &output_data(&result, false), RunFormat::Json).expect("render");
        let parsed: Value = serde_json::from_str(&out).expect("valid json");
        assert_eq!(parsed, json!([{"a": 1}, "b"]));
    }

    #[test]
    fn jsonl_format_renders_one_item_per_line() {
        let result = result_with(vec![json!({"a": 1}), json!("b")], ScriptStatus::Completed);
        let out = render(&result, &output_data(&result, false), RunFormat::Jsonl).expect("render");
        assert_eq!(out, "{\"a\":1}\n\"b\"\n");
    }

    #[test]
    fn jsonl_format_with_no_data_is_empty() {
        let result = result_with(Vec::new(), ScriptStatus::Completed);
        assert_eq!(
            render(&result, &output_data(&result, false), RunFormat::Jsonl).expect("render"),
            ""
        );
    }

    #[test]
    fn flatten_unwraps_one_level_of_arrays() {
        let result = result_with(
            vec![json!([1, 2]), json!([[3]]), json!("x")],
            ScriptStatus::Completed,
        );
        let out = render(&result, &output_data(&result, true), RunFormat::Jsonl).expect("render");
        assert_eq!(out, "1\n2\n[3]\n\"x\"\n");
    }

    #[test]
    fn result_format_includes_status_and_error() {
        let mut result = result_with(vec![json!([1])], ScriptStatus::Failed);
        result.error = Some("boom".to_string());
        let out = render(&result, &output_data(&result, true), RunFormat::Result).expect("render");
        let parsed: Value = serde_json::from_str(&out).expect("valid json");
        assert_eq!(parsed["status"], "Failed");
        assert_eq!(parsed["error"], "boom");
        assert_eq!(parsed["extracted_data"], json!([1]));
        assert_eq!(parsed["yielded_data"], json!(["page 1"]));
    }

    #[test]
    fn exit_codes_follow_script_status() {
        let code = |status| exit_code_for(&result_with(Vec::new(), status));
        assert_eq!(code(ScriptStatus::Completed), EXIT_OK);
        assert_eq!(code(ScriptStatus::Failed), EXIT_FAILED);
        assert_eq!(code(ScriptStatus::Cancelled), EXIT_INTERRUPTED);
    }

    #[test]
    fn summary_reports_counts_and_error() {
        let mut result = result_with(vec![json!(1)], ScriptStatus::Failed);
        result.error = Some("selector not found".to_string());
        // The count is what was written, which differs from the raw data
        // length when --flatten expands arrays.
        let line = summary_line(&result, 3);
        assert!(
            line.starts_with("script failed: 3 item(s), 2 step(s)"),
            "{line}"
        );
        assert!(line.contains("selector not found"), "{line}");
    }

    #[test]
    fn requested_limits_override_only_given_fields() {
        let options = RunOptions {
            script: "x".to_string(),
            format: RunFormat::Json,
            output: None,
            flatten: false,
            quiet: false,
            max_steps: Some(10),
            timeout_secs: None,
            step_timeout_secs: Some(5),
        };
        let limits = requested_limits(&options);
        assert_eq!(limits.max_steps, 10);
        assert_eq!(limits.per_step_timeout_secs, 5);
        assert_eq!(
            limits.max_timeout_secs,
            ScriptLimits::default().max_timeout_secs
        );
    }

    #[test]
    fn atomic_write_replaces_existing_file_and_leaves_no_temp() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("nested").join("out.json");
        write_atomically(&path, b"first").expect("first write");
        write_atomically(&path, b"second").expect("second write");
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "second");
        let leftovers: Vec<_> = std::fs::read_dir(dir.path().join("nested"))
            .expect("read dir")
            .map(|entry| entry.expect("entry").file_name())
            .collect();
        assert_eq!(leftovers, vec![std::ffi::OsString::from("out.json")]);
    }

    #[test]
    fn extension_error_is_actionable_and_never_suggests_a_fallback() {
        let message = not_connected_error(Duration::from_secs(3), 19876);
        assert!(message.contains("within 3s on port 19876"), "{message}");
        assert!(
            message.contains("acrawl config unset browser_backend"),
            "{message}"
        );
    }

    #[test]
    fn concurrent_writers_get_distinct_temp_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("out.json");
        let (first, _first_file) = create_unique_temp(&path).expect("first temp");
        let (second, _second_file) = create_unique_temp(&path).expect("second temp");
        assert_ne!(first, second);
        assert_eq!(first.parent(), Some(dir.path()));
    }
}
