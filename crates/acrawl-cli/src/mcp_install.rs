use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use dialoguer::{theme::ColorfulTheme, MultiSelect, Select};
use serde_json::{json, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ide {
    ClaudeCode,
    Cursor,
    Windsurf,
    VsCode,
    OpenCode,
}

impl Ide {
    const ALL: &[Self] = &[
        Self::ClaudeCode,
        Self::Cursor,
        Self::Windsurf,
        Self::VsCode,
        Self::OpenCode,
    ];

    fn name(self) -> &'static str {
        match self {
            Self::ClaudeCode => "Claude Code",
            Self::Cursor => "Cursor",
            Self::Windsurf => "Windsurf",
            Self::VsCode => "VS Code (Copilot)",
            Self::OpenCode => "OpenCode",
        }
    }

    fn supports_project_scope(self) -> bool {
        !matches!(self, Self::Windsurf)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scope {
    Global,
    Project,
}

struct DetectedIde {
    ide: Ide,
    reason: String,
}

fn home_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        env::var_os("USERPROFILE").map(PathBuf::from)
    }
    #[cfg(not(windows))]
    {
        env::var_os("HOME").map(PathBuf::from)
    }
}

fn command_exists(name: &str) -> bool {
    let check = if cfg!(windows) {
        Command::new("where").arg(name).output()
    } else {
        Command::new("which").arg(name).output()
    };
    check.is_ok_and(|o| o.status.success())
}

fn detect_ides() -> Vec<DetectedIde> {
    let mut detected = Vec::new();
    let home = home_dir();

    if command_exists("claude") {
        detected.push(DetectedIde {
            ide: Ide::ClaudeCode,
            reason: "claude CLI on PATH".to_string(),
        });
    }

    if let Some(ref h) = home {
        if h.join(".cursor").is_dir() {
            detected.push(DetectedIde {
                ide: Ide::Cursor,
                reason: "~/.cursor/ exists".to_string(),
            });
        }

        let windsurf_dir = h.join(".codeium").join("windsurf");
        if windsurf_dir.is_dir() {
            detected.push(DetectedIde {
                ide: Ide::Windsurf,
                reason: "~/.codeium/windsurf/ exists".to_string(),
            });
        }

        if h.join(".vscode").is_dir() || command_exists("code") {
            detected.push(DetectedIde {
                ide: Ide::VsCode,
                reason: "VS Code detected".to_string(),
            });
        }
    }

    if command_exists("opencode") {
        detected.push(DetectedIde {
            ide: Ide::OpenCode,
            reason: "opencode CLI on PATH".to_string(),
        });
    }

    detected
}

fn resolve_acrawl_path() -> String {
    let exe = env::current_exe().ok().and_then(|p| {
        match fs::canonicalize(&p) {
            Ok(canonical) => Some(canonical),
            Err(e) => {
                eprintln!(
                    "warning: could not canonicalize binary path {}: {e}",
                    p.display()
                );
                eprintln!(
                    "warning: IDE configs will use the bare name `acrawl` — \
                     ensure it is on PATH when IDEs launch the server"
                );
                None
            }
        }
    });
    let exe = exe.unwrap_or_else(|| PathBuf::from("acrawl"));

    let path_str = exe.to_string_lossy().to_string();
    path_str
        .strip_prefix(r"\\?\")
        .unwrap_or(&path_str)
        .replace('\\', "/")
}

fn prompt_ide_selection(detected: &[DetectedIde]) -> io::Result<Vec<Ide>> {
    let items: Vec<String> = Ide::ALL
        .iter()
        .map(|ide| {
            let reason = detected
                .iter()
                .find(|d| d.ide == *ide)
                .map(|d| format!(" ({})", d.reason))
                .unwrap_or_default();
            format!("{}{reason}", ide.name())
        })
        .collect();

    let defaults: Vec<bool> = Ide::ALL
        .iter()
        .map(|ide| detected.iter().any(|d| d.ide == *ide))
        .collect();

    let selections = MultiSelect::with_theme(&ColorfulTheme::default())
        .with_prompt("Select IDEs to configure (Space to toggle, Enter to confirm)")
        .items(&items)
        .defaults(&defaults)
        .interact_opt()
        .map_err(io::Error::other)?;

    match selections {
        Some(indices) => Ok(indices.into_iter().map(|i| Ide::ALL[i]).collect()),
        None => Ok(Vec::new()),
    }
}

fn prompt_scope() -> io::Result<Scope> {
    let items = &[
        "Global (user-level, works across all projects)",
        "Project (current directory, shareable via git)",
    ];

    let selection = Select::with_theme(&ColorfulTheme::default())
        .with_prompt("Config scope")
        .items(items)
        .default(0)
        .interact()
        .map_err(io::Error::other)?;

    match selection {
        1 => Ok(Scope::Project),
        _ => Ok(Scope::Global),
    }
}

fn merge_json_config(
    path: &Path,
    root_key: &str,
    server_name: &str,
    entry: Value,
) -> io::Result<()> {
    let existing: Value = if path.exists() {
        let content = fs::read_to_string(path)?;
        serde_json::from_str(&content).unwrap_or_else(|_| json!({}))
    } else {
        json!({})
    };

    let mut doc = existing.as_object().cloned().unwrap_or_default();
    let servers = doc
        .entry(root_key)
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{root_key} is not an object in {}", path.display()),
            )
        })?;
    servers.insert(server_name.to_string(), entry);

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let formatted = serde_json::to_string_pretty(&Value::Object(doc)).map_err(io::Error::other)?;
    fs::write(path, formatted.as_bytes())?;
    Ok(())
}

fn install_claude_code_global(acrawl_path: &str) -> io::Result<String> {
    if command_exists("claude") {
        let status = Command::new("claude")
            .args(["mcp", "add", "acrawl", "--", acrawl_path, "mcp"])
            .status()?;
        if status.success() {
            return Ok("configured via `claude mcp add`".to_string());
        }
        let _ = Command::new("claude")
            .args(["mcp", "remove", "acrawl"])
            .status();
        let retry = Command::new("claude")
            .args(["mcp", "add", "acrawl", "--", acrawl_path, "mcp"])
            .status()?;
        if retry.success() {
            return Ok("updated via `claude mcp add` (replaced existing)".to_string());
        }
    }
    let home = home_dir()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "cannot determine home dir"))?;
    let config_path = home.join(".claude.json");
    let entry = json!({
        "command": acrawl_path,
        "args": ["mcp"]
    });
    merge_json_config(&config_path, "mcpServers", "acrawl", entry)?;
    Ok(format!("wrote {}", config_path.display()))
}

fn install_claude_code_project(acrawl_path: &str) -> io::Result<String> {
    let config_path = PathBuf::from(".mcp.json");
    let entry = json!({
        "command": acrawl_path,
        "args": ["mcp"]
    });
    merge_json_config(&config_path, "mcpServers", "acrawl", entry)?;
    Ok(format!("wrote {}", config_path.display()))
}

fn install_cursor(acrawl_path: &str, scope: Scope) -> io::Result<String> {
    let config_path = match scope {
        Scope::Global => {
            let home = home_dir().ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "cannot determine home dir")
            })?;
            home.join(".cursor").join("mcp.json")
        }
        Scope::Project => PathBuf::from(".cursor").join("mcp.json"),
    };
    let entry = json!({
        "command": acrawl_path,
        "args": ["mcp"]
    });
    merge_json_config(&config_path, "mcpServers", "acrawl", entry)?;
    Ok(format!("wrote {}", config_path.display()))
}

fn install_windsurf(acrawl_path: &str) -> io::Result<String> {
    let home = home_dir()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "cannot determine home dir"))?;
    let config_path = home
        .join(".codeium")
        .join("windsurf")
        .join("mcp_config.json");
    let entry = json!({
        "command": acrawl_path,
        "args": ["mcp"]
    });
    merge_json_config(&config_path, "mcpServers", "acrawl", entry)?;
    Ok(format!("wrote {}", config_path.display()))
}

fn install_vscode(acrawl_path: &str, scope: Scope) -> io::Result<String> {
    let config_path = match scope {
        Scope::Global => {
            let home = home_dir().ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "cannot determine home dir")
            })?;
            home.join(".vscode").join("mcp.json")
        }
        Scope::Project => PathBuf::from(".vscode").join("mcp.json"),
    };
    let entry = json!({
        "command": acrawl_path,
        "args": ["mcp"]
    });
    merge_json_config(&config_path, "servers", "acrawl", entry)?;
    Ok(format!("wrote {}", config_path.display()))
}

fn global_opencode_config_path() -> PathBuf {
    home_dir()
        .unwrap_or_default()
        .join(".config")
        .join("opencode")
        .join("opencode.jsonc")
}

fn install_opencode(acrawl_path: &str, scope: Scope) -> io::Result<String> {
    let config_path = match scope {
        Scope::Global => global_opencode_config_path(),
        Scope::Project => PathBuf::from("opencode.json"),
    };

    let entry = json!({
        "type": "local",
        "command": [acrawl_path, "mcp"]
    });
    merge_json_config(&config_path, "mcp", "acrawl", entry)?;
    Ok(format!("wrote {}", config_path.display()))
}

fn install_for_ide(ide: Ide, scope: Scope, acrawl_path: &str) -> io::Result<String> {
    match ide {
        Ide::ClaudeCode => match scope {
            Scope::Global => install_claude_code_global(acrawl_path),
            Scope::Project => install_claude_code_project(acrawl_path),
        },
        Ide::Cursor => install_cursor(acrawl_path, scope),
        Ide::Windsurf => install_windsurf(acrawl_path),
        Ide::VsCode => install_vscode(acrawl_path, scope),
        Ide::OpenCode => install_opencode(acrawl_path, scope),
    }
}

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    let detected = detect_ides();

    if detected.is_empty() {
        eprintln!("No supported IDEs detected on this system.");
        eprintln!("Supported: Claude Code, Cursor, Windsurf, VS Code, OpenCode");
        eprintln!("\nYou can still select IDEs to configure manually.");
    }

    let selected = prompt_ide_selection(&detected)?;
    if selected.is_empty() {
        eprintln!("No IDEs selected. Nothing to do.");
        return Ok(());
    }

    let scope = prompt_scope()?;
    let acrawl_path = resolve_acrawl_path();

    eprintln!("\nInstalling acrawl MCP server (binary: {acrawl_path})...\n");

    let mut success_count = 0u32;
    for ide in &selected {
        if scope == Scope::Project && !ide.supports_project_scope() {
            eprintln!("  ⚠ {} — skipped (global config only)", ide.name());
            continue;
        }
        match install_for_ide(*ide, scope, &acrawl_path) {
            Ok(detail) => {
                eprintln!("  ✓ {} — {detail}", ide.name());
                success_count += 1;
            }
            Err(e) => {
                eprintln!("  ✗ {} — error: {e}", ide.name());
            }
        }
    }

    if success_count > 0 {
        eprintln!(
            "\nDone. acrawl MCP server configured for {success_count} IDE{}.",
            if success_count == 1 { "" } else { "s" }
        );
    } else {
        eprintln!("\nNo configurations were written.");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn resolve_acrawl_path_returns_non_empty() {
        let path = resolve_acrawl_path();
        assert!(!path.is_empty());
    }

    #[test]
    fn resolve_acrawl_path_uses_forward_slashes() {
        let path = resolve_acrawl_path();
        assert!(
            !path.contains('\\'),
            "path should use forward slashes: {path}"
        );
    }

    #[test]
    fn merge_json_config_creates_new_file() {
        let dir = env::temp_dir().join(format!("acrawl-mcp-install-test-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("test_mcp.json");

        let entry = json!({"command": "acrawl", "args": ["mcp"]});
        merge_json_config(&path, "mcpServers", "acrawl", entry.clone()).unwrap();

        let content: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(content["mcpServers"]["acrawl"], entry);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn merge_json_config_preserves_existing_servers() {
        let dir = env::temp_dir().join(format!("acrawl-mcp-install-merge-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("merge_test.json");

        fs::write(
            &path,
            r#"{"mcpServers":{"other":{"command":"other-server"}}}"#,
        )
        .unwrap();

        let entry = json!({"command": "acrawl", "args": ["mcp"]});
        merge_json_config(&path, "mcpServers", "acrawl", entry).unwrap();

        let content: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(content["mcpServers"]["other"]["command"], "other-server");
        assert_eq!(content["mcpServers"]["acrawl"]["command"], "acrawl");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn windsurf_does_not_support_project_scope() {
        assert!(!Ide::Windsurf.supports_project_scope());
    }

    #[test]
    fn all_other_ides_support_project_scope() {
        for ide in Ide::ALL {
            if *ide != Ide::Windsurf {
                assert!(
                    ide.supports_project_scope(),
                    "{ide:?} should support project scope"
                );
            }
        }
    }

    #[test]
    fn opencode_entry_uses_array_command() {
        let dir = env::temp_dir().join(format!("acrawl-mcp-install-oc-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("opencode.json");

        let entry = json!({"type": "local", "command": ["/usr/bin/acrawl", "mcp"]});
        merge_json_config(&path, "mcp", "acrawl", entry).unwrap();

        let content: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(content["mcp"]["acrawl"]["type"], "local");
        assert!(content["mcp"]["acrawl"]["command"].is_array());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn opencode_global_config_path_matches_documented_location() {
        let home = home_dir().expect("home dir should exist for test");
        let expected = home.join(".config").join("opencode").join("opencode.jsonc");

        let actual = global_opencode_config_path();

        assert_eq!(actual, expected);
    }

    #[test]
    fn vscode_uses_servers_root_key() {
        let dir = env::temp_dir().join(format!("acrawl-mcp-install-vsc-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("mcp.json");

        let entry = json!({"command": "acrawl", "args": ["mcp"]});
        merge_json_config(&path, "servers", "acrawl", entry).unwrap();

        let content: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert!(content.get("servers").is_some());
        assert!(content.get("mcpServers").is_none());

        let _ = fs::remove_dir_all(&dir);
    }
}
