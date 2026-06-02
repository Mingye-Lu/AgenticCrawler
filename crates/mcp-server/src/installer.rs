use std::env;
use std::fs;
use std::io;
use std::io::Write;
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
    ClaudeDesktop,
    JetBrains,
    Trae,
    GeminiCli,
    QwenCode,
    Crush,
    Zed,
    OpenClaw,
    CodexCli,
    Hermes,
    Goose,
    Aider,
}

impl Ide {
    const ALL: &[Self] = &[
        Self::ClaudeCode,
        Self::Cursor,
        Self::Windsurf,
        Self::VsCode,
        Self::OpenCode,
        Self::ClaudeDesktop,
        Self::JetBrains,
        Self::Trae,
        Self::GeminiCli,
        Self::QwenCode,
        Self::Crush,
        Self::Zed,
        Self::OpenClaw,
        Self::CodexCli,
        Self::Hermes,
        Self::Goose,
        Self::Aider,
    ];

    fn name(self) -> &'static str {
        match self {
            Self::ClaudeCode => "Claude Code",
            Self::Cursor => "Cursor",
            Self::Windsurf => "Windsurf",
            Self::VsCode => "VS Code (Copilot)",
            Self::OpenCode => "OpenCode",
            Self::ClaudeDesktop => "Claude Desktop",
            Self::JetBrains => "JetBrains IDEs",
            Self::Trae => "TRAE",
            Self::GeminiCli => "Gemini CLI",
            Self::QwenCode => "Qwen Code",
            Self::Crush => "Crush",
            Self::Zed => "Zed",
            Self::OpenClaw => "OpenClaw",
            Self::CodexCli => "Codex CLI",
            Self::Hermes => "Hermes",
            Self::Goose => "Goose",
            Self::Aider => "Aider",
        }
    }

    fn supports_project_scope(self) -> bool {
        !matches!(
            self,
            Self::Windsurf
                | Self::ClaudeDesktop
                | Self::JetBrains
                | Self::GeminiCli
                | Self::Zed
                | Self::OpenClaw
                | Self::CodexCli
                | Self::Hermes
                | Self::Goose
        )
    }

    fn supports_global_scope(self) -> bool {
        !matches!(self, Self::Trae | Self::Crush | Self::Aider)
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

    if command_exists("claude") {
        detected.push(DetectedIde {
            ide: Ide::ClaudeCode,
            reason: "claude CLI on PATH".to_string(),
        });
    }

    if let Some(home) = home_dir() {
        detected.extend(detect_home_dir_ides(&home));
    }

    if command_exists("opencode") {
        detected.push(DetectedIde {
            ide: Ide::OpenCode,
            reason: "opencode CLI on PATH".to_string(),
        });
    }

    if ["idea", "pycharm", "webstorm", "goland"]
        .into_iter()
        .find(|cmd| command_exists(cmd))
        .is_some()
    {
        detected.push(DetectedIde {
            ide: Ide::JetBrains,
            reason: "JetBrains launcher on PATH".to_string(),
        });
    }

    if command_exists("crush") {
        detected.push(DetectedIde {
            ide: Ide::Crush,
            reason: "crush CLI on PATH".to_string(),
        });
    }

    if command_exists("aider") {
        detected.push(DetectedIde {
            ide: Ide::Aider,
            reason: "aider CLI on PATH".to_string(),
        });
    }

    detected
}

fn detect_home_dir_ides(home: &Path) -> Vec<DetectedIde> {
    let mut detected = Vec::new();

    if home.join(".cursor").is_dir() {
        detected.push(DetectedIde {
            ide: Ide::Cursor,
            reason: "~/.cursor/ exists".to_string(),
        });
    }

    if home.join(".codeium").join("windsurf").is_dir() {
        detected.push(DetectedIde {
            ide: Ide::Windsurf,
            reason: "~/.codeium/windsurf/ exists".to_string(),
        });
    }

    if home.join(".vscode").is_dir() || command_exists("code") {
        detected.push(DetectedIde {
            ide: Ide::VsCode,
            reason: "VS Code detected".to_string(),
        });
    }

    if home.join(".trae").is_dir() {
        detected.push(DetectedIde {
            ide: Ide::Trae,
            reason: "~/.trae/ exists".to_string(),
        });
    }

    push_cli_or_dir_detection(
        &mut detected,
        Ide::GeminiCli,
        "gemini",
        home.join(".gemini").is_dir(),
        "~/.gemini/ exists",
    );
    push_cli_or_dir_detection(
        &mut detected,
        Ide::QwenCode,
        "qwen",
        home.join(".qwen").is_dir(),
        "~/.qwen/ exists",
    );
    push_cli_or_dir_detection(
        &mut detected,
        Ide::Zed,
        "zed",
        zed_config_path().parent().is_some_and(Path::is_dir),
        "Zed config directory exists",
    );
    push_cli_or_dir_detection(
        &mut detected,
        Ide::OpenClaw,
        "openclaw",
        home.join(".openclaw").is_dir(),
        "~/.openclaw/ exists",
    );
    push_cli_or_dir_detection(
        &mut detected,
        Ide::CodexCli,
        "codex",
        home.join(".codex").is_dir(),
        "~/.codex/ exists",
    );
    push_cli_or_dir_detection(
        &mut detected,
        Ide::Hermes,
        "hermes",
        home.join(".hermes").is_dir(),
        "~/.hermes/ exists",
    );
    push_cli_or_dir_detection(
        &mut detected,
        Ide::Goose,
        "goose",
        home.join(".config").join("goose").is_dir(),
        "~/.config/goose/ exists",
    );

    if claude_desktop_config_path()
        .parent()
        .is_some_and(Path::is_dir)
    {
        detected.push(DetectedIde {
            ide: Ide::ClaudeDesktop,
            reason: "Claude Desktop config directory exists".to_string(),
        });
    }

    detected
}

fn push_cli_or_dir_detection(
    detected: &mut Vec<DetectedIde>,
    ide: Ide,
    command: &str,
    has_dir: bool,
    dir_reason: &str,
) {
    let has_command = command_exists(command);
    if has_command || has_dir {
        detected.push(DetectedIde {
            ide,
            reason: if has_command {
                format!("{command} CLI on PATH")
            } else {
                dir_reason.to_string()
            },
        });
    }
}

fn standard_entry(acrawl_path: &str) -> Value {
    json!({
        "command": acrawl_path,
        "args": ["mcp"]
    })
}

fn supported_ide_names() -> String {
    Ide::ALL
        .iter()
        .map(|ide| ide.name())
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(windows)]
fn appdata_dir() -> Option<PathBuf> {
    env::var_os("APPDATA").map(PathBuf::from)
}

#[cfg(not(windows))]
fn appdata_dir() -> Option<PathBuf> {
    None
}

fn claude_desktop_config_path() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        return home_dir()
            .unwrap_or_default()
            .join("Library")
            .join("Application Support")
            .join("Claude")
            .join("claude_desktop_config.json");
    }
    #[cfg(windows)]
    {
        appdata_dir()
            .unwrap_or_default()
            .join("Claude")
            .join("claude_desktop_config.json")
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        home_dir()
            .unwrap_or_default()
            .join(".config")
            .join("Claude")
            .join("claude_desktop_config.json")
    }
}

fn zed_config_path() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        return home_dir()
            .unwrap_or_default()
            .join("Library")
            .join("Application Support")
            .join("Zed")
            .join("settings.json");
    }
    #[cfg(windows)]
    {
        appdata_dir()
            .unwrap_or_default()
            .join("Zed")
            .join("settings.json")
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        home_dir()
            .unwrap_or_default()
            .join(".config")
            .join("zed")
            .join("settings.json")
    }
}

fn openclaw_config_path() -> PathBuf {
    home_dir()
        .unwrap_or_default()
        .join(".openclaw")
        .join("openclaw.json")
}

fn goose_config_path() -> PathBuf {
    home_dir()
        .unwrap_or_default()
        .join(".config")
        .join("goose")
        .join("config.yaml")
}

fn install_standard_json_config(
    path: &Path,
    root_key: &str,
    acrawl_path: &str,
) -> io::Result<String> {
    merge_json_config(path, root_key, "acrawl", standard_entry(acrawl_path))?;
    Ok(format!("wrote {}", path.display()))
}

fn uninstall_standard_json_config(path: &Path, root_key: &str) -> io::Result<String> {
    if remove_json_config(path, root_key, "acrawl")? {
        Ok(format!("removed from {}", path.display()))
    } else {
        Ok(format!("not found in {}", path.display()))
    }
}

fn print_standard_json_snippet(acrawl_path: &str) {
    let snippet = serde_json::to_string_pretty(&json!({
        "acrawl": standard_entry(acrawl_path)
    }))
    .unwrap_or_else(|_| format!(r#"{{"acrawl":{{"command":"{acrawl_path}","args":["mcp"]}}}}"#));
    eprintln!("{snippet}");
}

fn resolve_acrawl_path() -> String {
    let exe = env::current_exe()
        .ok()
        .and_then(|p| match fs::canonicalize(&p) {
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
        // Always remove first (ignore errors — may not exist) to guarantee override.
        let _ = Command::new("claude")
            .args(["mcp", "remove", "acrawl"])
            .output();
        let status = Command::new("claude")
            .args(["mcp", "add", "acrawl", "--", acrawl_path, "mcp"])
            .status()?;
        if status.success() {
            return Ok("configured via `claude mcp add`".to_string());
        }
    }
    let home = home_dir()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "cannot determine home dir"))?;
    let config_path = home.join(".claude.json");
    install_standard_json_config(&config_path, "mcpServers", acrawl_path)
}

fn install_claude_code_project(acrawl_path: &str) -> io::Result<String> {
    let config_path = PathBuf::from(".mcp.json");
    install_standard_json_config(&config_path, "mcpServers", acrawl_path)
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
    install_standard_json_config(&config_path, "mcpServers", acrawl_path)
}

fn install_windsurf(acrawl_path: &str) -> io::Result<String> {
    let home = home_dir()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "cannot determine home dir"))?;
    let config_path = home
        .join(".codeium")
        .join("windsurf")
        .join("mcp_config.json");
    install_standard_json_config(&config_path, "mcpServers", acrawl_path)
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
    install_standard_json_config(&config_path, "servers", acrawl_path)
}

fn install_claude_desktop(acrawl_path: &str) -> io::Result<String> {
    let config_path = claude_desktop_config_path();
    install_standard_json_config(&config_path, "mcpServers", acrawl_path)
}

fn install_jetbrains(acrawl_path: &str) -> io::Result<String> {
    eprintln!("Add this server in JetBrains Settings > Tools > MCP Server:");
    print_standard_json_snippet(acrawl_path);
    io::stderr().flush()?;
    Ok("printed config to terminal (paste in JetBrains Settings > Tools > MCP Server)".to_string())
}

fn install_trae(acrawl_path: &str) -> io::Result<String> {
    let config_path = PathBuf::from(".trae").join("mcp.json");
    install_standard_json_config(&config_path, "mcpServers", acrawl_path)
}

fn install_gemini_cli(acrawl_path: &str) -> io::Result<String> {
    let home = home_dir()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "cannot determine home dir"))?;
    let config_path = home.join(".gemini").join("settings.json");
    install_standard_json_config(&config_path, "mcpServers", acrawl_path)
}

fn install_qwen_code(acrawl_path: &str, scope: Scope) -> io::Result<String> {
    let config_path = match scope {
        Scope::Global => home_dir()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "cannot determine home dir"))?
            .join(".qwen")
            .join("settings.json"),
        Scope::Project => PathBuf::from(".qwen").join("settings.json"),
    };
    install_standard_json_config(&config_path, "mcpServers", acrawl_path)
}

fn install_crush(acrawl_path: &str) -> io::Result<String> {
    let config_path = PathBuf::from(".crush.json");
    install_standard_json_config(&config_path, "mcpServers", acrawl_path)
}

fn install_zed(acrawl_path: &str) -> io::Result<String> {
    let config_path = zed_config_path();
    let entry = json!({
        "command": {
            "path": acrawl_path,
            "args": ["mcp"],
            "env": {}
        },
        "settings": {}
    });
    merge_json_config(&config_path, "context_servers", "acrawl", entry)?;
    Ok(format!("wrote {}", config_path.display()))
}

fn install_openclaw(acrawl_path: &str) -> io::Result<String> {
    let config_path = openclaw_config_path();
    let existing: Value = if config_path.exists() {
        let content = fs::read_to_string(&config_path)?;
        serde_json::from_str(&content).unwrap_or_else(|_| json!({}))
    } else {
        json!({})
    };
    let mut doc = existing.as_object().cloned().unwrap_or_default();
    let mcp = doc
        .entry("mcp")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("mcp is not an object in {}", config_path.display()),
            )
        })?;
    let servers = mcp
        .entry("servers")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("mcp.servers is not an object in {}", config_path.display()),
            )
        })?;
    servers.insert("acrawl".to_string(), standard_entry(acrawl_path));

    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let formatted = serde_json::to_string_pretty(&Value::Object(doc)).map_err(io::Error::other)?;
    fs::write(&config_path, formatted.as_bytes())?;
    Ok(format!("wrote {}", config_path.display()))
}

fn install_codex_cli(acrawl_path: &str) -> io::Result<String> {
    if !command_exists("codex") {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "codex CLI not found on PATH; run `codex mcp add acrawl -- <path-to-acrawl> mcp` manually",
        ));
    }

    let _ = Command::new("codex")
        .args(["mcp", "remove", "acrawl"])
        .output();
    let status = Command::new("codex")
        .args(["mcp", "add", "acrawl", "--", acrawl_path, "mcp"])
        .status()?;
    if status.success() {
        Ok("configured via `codex mcp add`".to_string())
    } else {
        Err(io::Error::other("`codex mcp add` failed"))
    }
}

fn install_hermes(acrawl_path: &str) -> io::Result<String> {
    if !command_exists("hermes") {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "hermes CLI not found on PATH; add acrawl manually in Hermes MCP settings",
        ));
    }

    let _ = Command::new("hermes")
        .args(["mcp", "remove", "acrawl"])
        .output();
    let status = Command::new("hermes")
        .args([
            "mcp",
            "add",
            "acrawl",
            "--command",
            acrawl_path,
            "--args",
            "mcp",
        ])
        .status()?;
    if status.success() {
        Ok("configured via `hermes mcp add`".to_string())
    } else {
        Err(io::Error::other("`hermes mcp add` failed"))
    }
}

fn install_goose(acrawl_path: &str) -> io::Result<String> {
    eprintln!("Add this server to {}:", goose_config_path().display());
    eprintln!("mcp_servers:");
    eprintln!("  acrawl:");
    eprintln!("    command: {acrawl_path}");
    eprintln!("    args:");
    eprintln!("      - mcp");
    io::stderr().flush()?;
    Ok("printed config snippet (add to ~/.config/goose/config.yaml)".to_string())
}

fn install_aider(acrawl_path: &str) -> io::Result<String> {
    install_claude_code_project(acrawl_path)
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
        Ide::ClaudeDesktop => install_claude_desktop(acrawl_path),
        Ide::JetBrains => install_jetbrains(acrawl_path),
        Ide::Trae => install_trae(acrawl_path),
        Ide::GeminiCli => install_gemini_cli(acrawl_path),
        Ide::QwenCode => install_qwen_code(acrawl_path, scope),
        Ide::Crush => install_crush(acrawl_path),
        Ide::Zed => install_zed(acrawl_path),
        Ide::OpenClaw => install_openclaw(acrawl_path),
        Ide::CodexCli => install_codex_cli(acrawl_path),
        Ide::Hermes => install_hermes(acrawl_path),
        Ide::Goose => install_goose(acrawl_path),
        Ide::Aider => install_aider(acrawl_path),
    }
}

fn remove_json_config(path: &Path, root_key: &str, server_name: &str) -> io::Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    let content = fs::read_to_string(path)?;
    let existing: Value = serde_json::from_str(&content).unwrap_or_else(|_| json!({}));
    let mut doc = existing.as_object().cloned().unwrap_or_default();

    let removed = doc
        .get_mut(root_key)
        .and_then(|v| v.as_object_mut())
        .is_some_and(|servers| servers.remove(server_name).is_some());

    if removed {
        let formatted =
            serde_json::to_string_pretty(&Value::Object(doc)).map_err(io::Error::other)?;
        fs::write(path, formatted.as_bytes())?;
    }

    Ok(removed)
}

fn uninstall_claude_code_global() -> io::Result<String> {
    if command_exists("claude") {
        let status = Command::new("claude")
            .args(["mcp", "remove", "acrawl"])
            .status()?;
        if status.success() {
            return Ok("removed via `claude mcp remove`".to_string());
        }
    }
    let home = home_dir()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "cannot determine home dir"))?;
    let config_path = home.join(".claude.json");
    if remove_json_config(&config_path, "mcpServers", "acrawl")? {
        Ok(format!("removed from {}", config_path.display()))
    } else {
        Ok(format!("not found in {}", config_path.display()))
    }
}

fn uninstall_claude_code_project() -> io::Result<String> {
    uninstall_standard_json_config(Path::new(".mcp.json"), "mcpServers")
}

fn uninstall_cursor(scope: Scope) -> io::Result<String> {
    let config_path = match scope {
        Scope::Global => {
            let home = home_dir().ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "cannot determine home dir")
            })?;
            home.join(".cursor").join("mcp.json")
        }
        Scope::Project => PathBuf::from(".cursor").join("mcp.json"),
    };
    uninstall_standard_json_config(&config_path, "mcpServers")
}

fn uninstall_windsurf() -> io::Result<String> {
    let home = home_dir()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "cannot determine home dir"))?;
    let config_path = home
        .join(".codeium")
        .join("windsurf")
        .join("mcp_config.json");
    uninstall_standard_json_config(&config_path, "mcpServers")
}

fn uninstall_vscode(scope: Scope) -> io::Result<String> {
    let config_path = match scope {
        Scope::Global => {
            let home = home_dir().ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "cannot determine home dir")
            })?;
            home.join(".vscode").join("mcp.json")
        }
        Scope::Project => PathBuf::from(".vscode").join("mcp.json"),
    };
    uninstall_standard_json_config(&config_path, "servers")
}

fn uninstall_opencode(scope: Scope) -> io::Result<String> {
    let config_path = match scope {
        Scope::Global => global_opencode_config_path(),
        Scope::Project => PathBuf::from("opencode.json"),
    };
    if remove_json_config(&config_path, "mcp", "acrawl")? {
        Ok(format!("removed from {}", config_path.display()))
    } else {
        Ok(format!("not found in {}", config_path.display()))
    }
}

fn uninstall_claude_desktop() -> io::Result<String> {
    let config_path = claude_desktop_config_path();
    uninstall_standard_json_config(&config_path, "mcpServers")
}

fn uninstall_jetbrains() -> io::Result<String> {
    eprintln!("Remove the `acrawl` MCP server from JetBrains Settings > Tools > MCP Server.");
    io::stderr().flush()?;
    Ok(
        "printed removal instructions (remove from JetBrains Settings > Tools > MCP Server)"
            .to_string(),
    )
}

fn uninstall_trae() -> io::Result<String> {
    let config_path = PathBuf::from(".trae").join("mcp.json");
    uninstall_standard_json_config(&config_path, "mcpServers")
}

fn uninstall_gemini_cli() -> io::Result<String> {
    let home = home_dir()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "cannot determine home dir"))?;
    let config_path = home.join(".gemini").join("settings.json");
    uninstall_standard_json_config(&config_path, "mcpServers")
}

fn uninstall_qwen_code(scope: Scope) -> io::Result<String> {
    let config_path = match scope {
        Scope::Global => home_dir()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "cannot determine home dir"))?
            .join(".qwen")
            .join("settings.json"),
        Scope::Project => PathBuf::from(".qwen").join("settings.json"),
    };
    uninstall_standard_json_config(&config_path, "mcpServers")
}

fn uninstall_crush() -> io::Result<String> {
    uninstall_standard_json_config(Path::new(".crush.json"), "mcpServers")
}

fn uninstall_zed() -> io::Result<String> {
    let config_path = zed_config_path();
    uninstall_standard_json_config(&config_path, "context_servers")
}

fn uninstall_openclaw() -> io::Result<String> {
    let config_path = openclaw_config_path();
    if !config_path.exists() {
        return Ok(format!("not found in {}", config_path.display()));
    }

    let content = fs::read_to_string(&config_path)?;
    let existing: Value = serde_json::from_str(&content).unwrap_or_else(|_| json!({}));
    let mut doc = existing.as_object().cloned().unwrap_or_default();

    let removed = doc
        .get_mut("mcp")
        .and_then(|value| value.as_object_mut())
        .and_then(|mcp| mcp.get_mut("servers"))
        .and_then(|value| value.as_object_mut())
        .is_some_and(|servers| servers.remove("acrawl").is_some());

    if removed {
        let formatted =
            serde_json::to_string_pretty(&Value::Object(doc)).map_err(io::Error::other)?;
        fs::write(&config_path, formatted.as_bytes())?;
        Ok(format!("removed from {}", config_path.display()))
    } else {
        Ok(format!("not found in {}", config_path.display()))
    }
}

fn uninstall_codex_cli() -> io::Result<String> {
    if !command_exists("codex") {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "codex CLI not found on PATH; run `codex mcp remove acrawl` manually",
        ));
    }

    let status = Command::new("codex")
        .args(["mcp", "remove", "acrawl"])
        .status()?;
    if status.success() {
        Ok("removed via `codex mcp remove`".to_string())
    } else {
        Err(io::Error::other("`codex mcp remove` failed"))
    }
}

fn uninstall_hermes() -> io::Result<String> {
    if !command_exists("hermes") {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "hermes CLI not found on PATH; remove acrawl manually in Hermes MCP settings",
        ));
    }

    let status = Command::new("hermes")
        .args(["mcp", "remove", "acrawl"])
        .status()?;
    if status.success() {
        Ok("removed via `hermes mcp remove`".to_string())
    } else {
        Err(io::Error::other("`hermes mcp remove` failed"))
    }
}

fn uninstall_goose() -> io::Result<String> {
    eprintln!(
        "Remove the `acrawl` block manually from {}.",
        goose_config_path().display()
    );
    io::stderr().flush()?;
    Ok("printed removal instructions (remove from ~/.config/goose/config.yaml)".to_string())
}

fn uninstall_aider() -> io::Result<String> {
    uninstall_claude_code_project()
}

fn uninstall_for_ide(ide: Ide, scope: Scope) -> io::Result<String> {
    match ide {
        Ide::ClaudeCode => match scope {
            Scope::Global => uninstall_claude_code_global(),
            Scope::Project => uninstall_claude_code_project(),
        },
        Ide::Cursor => uninstall_cursor(scope),
        Ide::Windsurf => uninstall_windsurf(),
        Ide::VsCode => uninstall_vscode(scope),
        Ide::OpenCode => uninstall_opencode(scope),
        Ide::ClaudeDesktop => uninstall_claude_desktop(),
        Ide::JetBrains => uninstall_jetbrains(),
        Ide::Trae => uninstall_trae(),
        Ide::GeminiCli => uninstall_gemini_cli(),
        Ide::QwenCode => uninstall_qwen_code(scope),
        Ide::Crush => uninstall_crush(),
        Ide::Zed => uninstall_zed(),
        Ide::OpenClaw => uninstall_openclaw(),
        Ide::CodexCli => uninstall_codex_cli(),
        Ide::Hermes => uninstall_hermes(),
        Ide::Goose => uninstall_goose(),
        Ide::Aider => uninstall_aider(),
    }
}

pub fn run_uninstall() -> Result<(), Box<dyn std::error::Error>> {
    let detected = detect_ides();

    if detected.is_empty() {
        eprintln!("No supported IDEs detected on this system.");
        eprintln!("Supported: {}", supported_ide_names());
        eprintln!("\nYou can still select IDEs to unconfigure manually.");
    }

    let selected = prompt_ide_selection(&detected)?;
    if selected.is_empty() {
        eprintln!("No IDEs selected. Nothing to do.");
        return Ok(());
    }

    let scope = prompt_scope()?;

    eprintln!("\nRemoving acrawl MCP server configuration...\n");

    let mut success_count = 0u32;
    for ide in &selected {
        if scope == Scope::Global && !ide.supports_global_scope() {
            eprintln!("  ⚠ {} — skipped (project-level config only)", ide.name());
            continue;
        }
        if scope == Scope::Project && !ide.supports_project_scope() {
            eprintln!("  ⚠ {} — skipped (global config only)", ide.name());
            continue;
        }
        match uninstall_for_ide(*ide, scope) {
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
            "\nDone. acrawl MCP server removed from {success_count} IDE{}.",
            if success_count == 1 { "" } else { "s" }
        );
    } else {
        eprintln!("\nNo configurations were removed.");
    }
    Ok(())
}

pub fn run_install() -> Result<(), Box<dyn std::error::Error>> {
    let detected = detect_ides();

    if detected.is_empty() {
        eprintln!("No supported IDEs detected on this system.");
        eprintln!("Supported: {}", supported_ide_names());
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
        if scope == Scope::Global && !ide.supports_global_scope() {
            eprintln!("  ⚠ {} — skipped (project-level config only)", ide.name());
            continue;
        }
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
            if !matches!(
                ide,
                Ide::Windsurf
                    | Ide::ClaudeDesktop
                    | Ide::JetBrains
                    | Ide::GeminiCli
                    | Ide::Zed
                    | Ide::OpenClaw
                    | Ide::CodexCli
                    | Ide::Hermes
                    | Ide::Goose
            ) {
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

    #[test]
    fn project_only_ides_do_not_support_global_scope() {
        for ide in [Ide::Trae, Ide::Crush, Ide::Aider] {
            assert!(
                !ide.supports_global_scope(),
                "{ide:?} should be project-only"
            );
        }
    }

    #[test]
    fn all_other_ides_support_global_scope() {
        for ide in Ide::ALL {
            if !matches!(ide, Ide::Trae | Ide::Crush | Ide::Aider) {
                assert!(
                    ide.supports_global_scope(),
                    "{ide:?} should support global scope"
                );
            }
        }
    }

    #[test]
    fn openclaw_install_uses_nested_servers_key() {
        let dir = env::temp_dir().join(format!(
            "acrawl-mcp-install-openclaw-{}",
            std::process::id()
        ));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("openclaw.json");

        let existing = json!({"mcp": {"servers": {"other": {"command": "other"}}}});
        fs::write(&path, serde_json::to_string(&existing).unwrap()).unwrap();

        let content = fs::read_to_string(&path).unwrap();
        let existing: Value = serde_json::from_str(&content).unwrap();
        let mut doc = existing.as_object().cloned().unwrap_or_default();
        let mcp = doc
            .entry("mcp")
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .unwrap();
        let servers = mcp
            .entry("servers")
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .unwrap();
        servers.insert("acrawl".to_string(), standard_entry("acrawl"));
        fs::write(
            &path,
            serde_json::to_string_pretty(&Value::Object(doc)).unwrap(),
        )
        .unwrap();

        let content: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(content["mcp"]["servers"]["other"]["command"], "other");
        assert_eq!(content["mcp"]["servers"]["acrawl"]["command"], "acrawl");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn zed_entry_uses_context_servers_root_key() {
        let dir = env::temp_dir().join(format!("acrawl-mcp-install-zed-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("settings.json");

        let entry = json!({
            "command": {
                "path": "acrawl",
                "args": ["mcp"],
                "env": {}
            },
            "settings": {}
        });
        merge_json_config(&path, "context_servers", "acrawl", entry).unwrap();

        let content: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert!(content.get("context_servers").is_some());
        assert_eq!(
            content["context_servers"]["acrawl"]["command"]["path"],
            "acrawl"
        );

        let _ = fs::remove_dir_all(&dir);
    }
}
