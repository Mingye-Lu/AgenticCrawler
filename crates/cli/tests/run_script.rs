//! `acrawl run` must reject bad scripts before it launches a browser, so these
//! run without Node or Chromium and finish in milliseconds.

use std::process::{Command, Output};

fn run_acrawl(config_home: &std::path::Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_acrawl"))
        .env("ACRAWL_CONFIG_HOME", config_home)
        .arg("run")
        .args(args)
        .output()
        .expect("failed to spawn acrawl")
}

#[test]
fn missing_saved_script_exits_with_usage_code() {
    let home = tempfile::tempdir().expect("tempdir");
    let output = run_acrawl(home.path(), &["does_not_exist"]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "stderr: {stderr}");
    assert!(stderr.contains("does_not_exist"), "{stderr}");
    assert!(output.stdout.is_empty());
}

#[test]
fn invalid_script_file_exits_before_browser_launch() {
    let home = tempfile::tempdir().expect("tempdir");
    let script = home.path().join("bad.json");
    std::fs::write(
        &script,
        r#"{"schema_version":1,"steps":[{"type":"collect","value":{"kind":"variable","value":"ghost"}}]}"#,
    )
    .expect("write script");

    let output = run_acrawl(home.path(), &[script.to_str().expect("utf8 path")]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "stderr: {stderr}");
    assert!(stderr.contains("ghost"), "{stderr}");
    assert!(!stderr.contains("launch browser"), "{stderr}");
}

#[test]
fn unknown_run_option_is_a_usage_error() {
    let home = tempfile::tempdir().expect("tempdir");
    let output = run_acrawl(home.path(), &["x", "--format", "csv"]);
    assert_eq!(output.status.code(), Some(2));
}
