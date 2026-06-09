use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use acrawl_core::{script_types::ScriptLimits, ToolEffect};
use agent::{mvp_tool_specs, tools};
use serde_json::{json, Value};
use tempfile::TempDir;

fn script_fixture() -> Value {
    json!({
        "schema_version": 1,
        "steps": []
    })
}

fn script_limits_fixture() -> Value {
    json!({
        "max_steps": 50,
        "max_timeout_secs": 60,
        "max_output_bytes": 1024,
        "max_parallel_branches": 2,
        "per_step_timeout_secs": 10,
        "max_script_size_bytes": 2048,
        "max_nesting_depth": 3
    })
}

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn with_temp_config_home(test: impl FnOnce(&Path)) {
    let _guard = env_lock().lock().expect("env lock poisoned");
    let temp_dir = TempDir::new().expect("temp dir should be created");
    let original = std::env::var_os("ACRAWL_CONFIG_HOME");
    std::env::set_var("ACRAWL_CONFIG_HOME", temp_dir.path());
    test(temp_dir.path());
    match original {
        Some(value) => std::env::set_var("ACRAWL_CONFIG_HOME", value),
        None => std::env::remove_var("ACRAWL_CONFIG_HOME"),
    }
}

fn unwrap_reply(effect: ToolEffect) -> String {
    match effect {
        ToolEffect::Reply(reply) => reply,
        other => panic!("expected Reply effect, got {other:?}"),
    }
}

fn saved_script_path(config_home: &Path, name: &str) -> PathBuf {
    config_home.join("scripts").join(format!("{name}.json"))
}

#[test]
fn script_integration_test_run_script_spawns_and_returns_id() {
    let effect = tools::run_script::execute(&json!({
        "script": script_fixture(),
        "save_as": "saved-script",
        "limits": script_limits_fixture()
    }))
    .expect("run_script should parse valid input");

    match effect {
        ToolEffect::RunScript(task) => {
            assert_eq!(task.script, script_fixture());
            assert_eq!(task.save_as.as_deref(), Some("saved-script"));
            assert_eq!(task.limits.max_steps, 50);
            assert_eq!(task.limits.max_timeout_secs, 60);
            assert_eq!(task.limits.max_output_bytes, 1024);
            assert_eq!(task.limits.max_parallel_branches, 2);
            assert_eq!(task.limits.per_step_timeout_secs, 10);
            assert_eq!(task.limits.max_script_size_bytes, 2048);
            assert_eq!(task.limits.max_nesting_depth, 3);
        }
        other => panic!("expected RunScript effect, got {other:?}"),
    }
}

#[test]
fn script_integration_test_script_status_tool_returns_effect() {
    let effect = tools::script_status::execute(&json!({ "script_id": "scr_deadbeef" }))
        .expect("script_status should parse valid input");

    match effect {
        ToolEffect::ScriptStatus(spec) => assert_eq!(spec.script_id, "scr_deadbeef"),
        other => panic!("expected ScriptStatus effect, got {other:?}"),
    }
}

#[test]
fn script_integration_test_wait_for_scripts_tool_returns_effect() {
    let effect = tools::wait_for_scripts::execute(&json!({
        "script_ids": ["scr_one", "scr_two"]
    }))
    .expect("wait_for_scripts should parse valid input");

    match effect {
        ToolEffect::ScriptWait(spec) => {
            assert_eq!(
                spec.script_ids,
                Some(vec!["scr_one".to_string(), "scr_two".to_string()])
            );
        }
        other => panic!("expected ScriptWait effect, got {other:?}"),
    }
}

#[test]
fn script_integration_test_cancel_script_tool_returns_effect() {
    let effect = tools::cancel_script::execute(&json!({ "script_id": "scr_cancelme" }))
        .expect("cancel_script should parse valid input");

    match effect {
        ToolEffect::ScriptCancel(spec) => assert_eq!(spec.script_id, "scr_cancelme"),
        other => panic!("expected ScriptCancel effect, got {other:?}"),
    }
}

#[test]
fn script_integration_test_save_script_writes_to_disk() {
    with_temp_config_home(|config_home| {
        let effect = tools::save_script::execute(&json!({
            "name": "saved_script",
            "script": script_fixture()
        }))
        .expect("save_script should write a valid script");

        assert_eq!(unwrap_reply(effect), "Script 'saved_script' saved");

        let script_path = saved_script_path(config_home, "saved_script");
        assert!(script_path.exists(), "saved script file should exist");

        let content = fs::read_to_string(script_path).expect("saved script should be readable");
        let parsed: Value = serde_json::from_str(&content).expect("saved script should be JSON");
        assert_eq!(parsed, script_fixture());
    });
}

#[test]
fn script_integration_test_list_scripts_returns_empty_for_new_dir() {
    with_temp_config_home(|_config_home| {
        let effect = tools::list_scripts::execute(&json!({}))
            .expect("list_scripts should handle missing scripts dir");

        let parsed: Value = serde_json::from_str(&unwrap_reply(effect))
            .expect("list_scripts reply should be valid JSON");
        assert_eq!(parsed, json!([]));
    });
}

#[test]
fn script_integration_test_read_script_reads_saved_file() {
    with_temp_config_home(|config_home| {
        let scripts_dir = config_home.join("scripts");
        fs::create_dir_all(&scripts_dir).expect("scripts dir should be created");
        let script_json =
            serde_json::to_string_pretty(&script_fixture()).expect("fixture should serialize");
        fs::write(saved_script_path(config_home, "reader"), &script_json)
            .expect("script fixture should be written");

        let effect = tools::read_script::execute(&json!({ "name": "reader" }))
            .expect("read_script should read saved file");

        let parsed: Value = serde_json::from_str(&unwrap_reply(effect))
            .expect("read_script reply should be valid JSON");
        assert_eq!(parsed, script_fixture());
    });
}

#[test]
fn script_integration_test_tool_handlers_reject_missing_required_fields() {
    let run_script_err = tools::run_script::execute(&json!({}))
        .expect_err("run_script should require script or name");
    assert!(run_script_err
        .to_string()
        .contains("requires either `script` or `name`"));

    let script_status_err = tools::script_status::execute(&json!({}))
        .expect_err("script_status should require script_id");
    assert!(script_status_err.to_string().contains("script_id"));

    let cancel_script_err = tools::cancel_script::execute(&json!({}))
        .expect_err("cancel_script should require script_id");
    assert!(cancel_script_err.to_string().contains("script_id"));

    let read_script_err =
        tools::read_script::execute(&json!({})).expect_err("read_script should require name");
    assert!(read_script_err.to_string().contains("name"));
}

#[test]
fn script_integration_test_save_script_rejects_path_traversal() {
    let err = tools::save_script::execute(&json!({
        "name": "nested/script",
        "script": script_fixture()
    }))
    .expect_err("save_script should reject path traversal");

    assert!(err.to_string().contains("path traversal"));
}

#[test]
fn script_integration_test_script_tool_schemas_are_valid() {
    let specs = mvp_tool_specs();
    let tool_names = [
        "run_script",
        "script_status",
        "wait_for_scripts",
        "cancel_script",
        "save_script",
        "list_scripts",
        "read_script",
    ];

    for tool_name in tool_names {
        let spec = specs
            .iter()
            .find(|spec| spec.name == tool_name)
            .unwrap_or_else(|| panic!("missing tool spec for {tool_name}"));

        assert_eq!(
            spec.input_schema["type"],
            json!("object"),
            "tool: {tool_name}"
        );
        assert!(
            spec.input_schema["properties"].is_object(),
            "tool: {tool_name}"
        );
        assert_eq!(
            spec.input_schema["additionalProperties"],
            json!(false),
            "tool: {tool_name}"
        );
    }

    let required_fields = [
        ("script_status", "script_id"),
        ("cancel_script", "script_id"),
        ("save_script", "name"),
        ("save_script", "script"),
        ("read_script", "name"),
    ];

    for (tool_name, required_field) in required_fields {
        let spec = specs
            .iter()
            .find(|spec| spec.name == tool_name)
            .unwrap_or_else(|| panic!("missing tool spec for {tool_name}"));
        let required = spec.input_schema["required"]
            .as_array()
            .unwrap_or_else(|| panic!("tool {tool_name} should declare required fields"));
        assert!(
            required.iter().any(|field| field == required_field),
            "tool {tool_name} should require {required_field}"
        );
    }
}

#[test]
fn script_integration_test_run_script_uses_default_limits_without_override() {
    let effect = tools::run_script::execute(&json!({
        "script": script_fixture()
    }))
    .expect("run_script should use default limits");

    match effect {
        ToolEffect::RunScript(task) => {
            let expected = ScriptLimits {
                max_steps: 200,
                max_timeout_secs: 300,
                max_output_bytes: 10_485_760,
                max_parallel_branches: 10,
                per_step_timeout_secs: 30,
                max_script_size_bytes: 1_048_576,
                max_nesting_depth: 10,
            };
            assert_eq!(task.limits.max_steps, expected.max_steps);
            assert_eq!(task.limits.max_timeout_secs, expected.max_timeout_secs);
            assert_eq!(task.limits.max_output_bytes, expected.max_output_bytes);
            assert_eq!(
                task.limits.max_parallel_branches,
                expected.max_parallel_branches
            );
            assert_eq!(
                task.limits.per_step_timeout_secs,
                expected.per_step_timeout_secs
            );
            assert_eq!(
                task.limits.max_script_size_bytes,
                expected.max_script_size_bytes
            );
            assert_eq!(task.limits.max_nesting_depth, expected.max_nesting_depth);
        }
        other => panic!("expected RunScript effect, got {other:?}"),
    }
}
