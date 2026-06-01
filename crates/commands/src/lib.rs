//! Slash command registry and parsing for the acrawl REPL.
//!
//! This crate provides the canonical set of 17 slash commands available in the interactive REPL,
//! along with parsing, help rendering, and execution logic. Each command is defined as a
//! [`SlashCommandSpec`] with metadata including name, summary, optional argument hints, and
//! whether it is resume-safe.
//!
//! ## Resume-Safe Commands
//!
//! A subset of 8 commands are marked `resume_supported: true`, meaning they can be safely
//! replayed when resuming a saved session via `--resume SESSION.json`. These include:
//! `/help`, `/status`, `/compact`, `/clear`, `/cost`, `/config`, `/version`, and `/export`.
//!
//! Commands that are not resume-safe (e.g., `/model`, `/sessions`, `/auth`, `/headed`,
//! `/headless`, `/debug`, `/exit`) are skipped during session replay because they either:
//! - Require user interaction or runtime state (e.g., `/model` to switch providers)
//! - Are only meaningful in the live REPL (e.g., `/sessions` opens an interactive picker)
//! - Modify browser or authentication state that should not be replayed
//!
//! ## Command Registry Pattern
//!
//! The module exports:
//! - [`slash_command_specs()`] — returns the full 17-command spec list
//! - [`resume_supported_slash_commands()`] — filters to the 8 resume-safe commands
//! - [`SlashCommand::parse()`] — parses user input into a [`SlashCommand`] enum
//! - [`handle_slash_command()`] — executes a command and returns a [`SlashCommandResult`]
//! - [`render_slash_command_help()`] — generates the help text shown by `/help`

use runtime::{compact_session, CompactionConfig, Session};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandManifestEntry {
    pub name: String,
    pub source: CommandSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandSource {
    Builtin,
    InternalOnly,
    FeatureGated,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommandRegistry {
    entries: Vec<CommandManifestEntry>,
}

impl CommandRegistry {
    #[must_use]
    pub fn new(entries: Vec<CommandManifestEntry>) -> Self {
        Self { entries }
    }

    #[must_use]
    pub fn entries(&self) -> &[CommandManifestEntry] {
        &self.entries
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlashCommandSpec {
    pub name: &'static str,
    pub summary: &'static str,
    pub argument_hint: Option<&'static str>,
    pub resume_supported: bool,
}

const SLASH_COMMAND_SPECS: &[SlashCommandSpec] = &[
    SlashCommandSpec {
        name: "help",
        summary: "Show available slash commands",
        argument_hint: None,
        resume_supported: true,
    },
    SlashCommandSpec {
        name: "status",
        summary: "Show current session status",
        argument_hint: None,
        resume_supported: true,
    },
    SlashCommandSpec {
        name: "compact",
        summary: "Compact local session history",
        argument_hint: None,
        resume_supported: true,
    },
    SlashCommandSpec {
        name: "model",
        summary: "Show or switch the active model",
        argument_hint: Some("[model]"),
        resume_supported: false,
    },
    SlashCommandSpec {
        name: "clear",
        summary: "Start a fresh local session",
        argument_hint: None,
        resume_supported: true,
    },
    SlashCommandSpec {
        name: "cost",
        summary: "Show cumulative token usage for this session",
        argument_hint: None,
        resume_supported: true,
    },
    SlashCommandSpec {
        name: "config",
        summary: "Inspect acrawl config files or merged sections",
        argument_hint: Some("[model]"),
        resume_supported: true,
    },
    SlashCommandSpec {
        name: "version",
        summary: "Show CLI version and build information",
        argument_hint: None,
        resume_supported: true,
    },
    SlashCommandSpec {
        name: "debug",
        summary: "Show debug details for the last browser tool call",
        argument_hint: None,
        resume_supported: false,
    },
    SlashCommandSpec {
        name: "export",
        summary: "Export the current conversation to a file",
        argument_hint: Some("[file]"),
        resume_supported: true,
    },
    SlashCommandSpec {
        name: "sessions",
        summary: "Open the session picker (TUI)",
        argument_hint: None,
        resume_supported: false,
    },
    SlashCommandSpec {
        name: "auth",
        summary: "Authenticate with a provider (auto-launched if no credentials)",
        argument_hint: Some("[anthropic|openai|other]"),
        resume_supported: false,
    },
    SlashCommandSpec {
        name: "headed",
        summary: "Switch browser to headed (visible) mode",
        argument_hint: None,
        resume_supported: false,
    },
    SlashCommandSpec {
        name: "headless",
        summary: "Switch browser to headless mode",
        argument_hint: None,
        resume_supported: false,
    },
    SlashCommandSpec {
        name: "extension",
        summary: "Start/show the extension bridge, or stop it",
        argument_hint: Some("[stop]"),
        resume_supported: false,
    },
    SlashCommandSpec {
        name: "cloakbrowser",
        summary: "Switch back to CloakBrowser backend",
        argument_hint: None,
        resume_supported: false,
    },
    SlashCommandSpec {
        name: "exit",
        summary: "Exit the REPL and save the session",
        argument_hint: None,
        resume_supported: false,
    },
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlashCommand {
    Help,
    Status,
    Compact,
    Debug,
    Model { model: Option<String> },
    Clear,
    Cost,
    Config { section: Option<String> },
    Version,
    Export { path: Option<String> },
    Sessions,
    Auth { provider: Option<String> },
    Headed,
    Headless,
    Extension { stop: bool },
    CloakBrowser,
    Unknown(String),
}

impl SlashCommand {
    #[must_use]
    pub fn parse(input: &str) -> Option<Self> {
        let trimmed = input.trim();
        if !trimmed.starts_with('/') {
            return None;
        }

        let mut parts = trimmed.trim_start_matches('/').split_whitespace();
        let command_raw = parts.next().unwrap_or_default();
        let command_lower = command_raw.to_ascii_lowercase();
        let command = command_lower.as_str();
        Some(match command {
            "help" => Self::Help,
            "status" => Self::Status,
            "compact" => Self::Compact,
            "debug" => Self::Debug,
            "model" => Self::Model {
                model: parts.next().map(ToOwned::to_owned),
            },
            "clear" => {
                // /clear takes no arguments. Any trailing args are rejected.
                let next = parts.next();
                if next.is_some() {
                    Self::Unknown(command_raw.to_string())
                } else {
                    Self::Clear
                }
            }
            "cost" => Self::Cost,
            "config" => Self::Config {
                section: parts.next().map(ToOwned::to_owned),
            },
            "version" => Self::Version,
            "export" => Self::Export {
                path: parts.next().map(ToOwned::to_owned),
            },
            "sessions" => Self::Sessions,
            "auth" => Self::Auth {
                provider: parts.next().map(ToOwned::to_owned),
            },
            "headed" => Self::Headed,
            "headless" => Self::Headless,
            "extension" => {
                let next = parts.next();
                let extra = parts.next();
                match (next, extra) {
                    (None, _) => Self::Extension { stop: false },
                    (Some("stop"), None) => Self::Extension { stop: true },
                    _ => Self::Unknown(command_raw.to_string()),
                }
            }
            "cloakbrowser" => Self::CloakBrowser,
            other => Self::Unknown(other.to_string()),
        })
    }
}

#[must_use]
pub fn slash_command_specs() -> &'static [SlashCommandSpec] {
    SLASH_COMMAND_SPECS
}

#[must_use]
pub fn resume_supported_slash_commands() -> Vec<&'static SlashCommandSpec> {
    slash_command_specs()
        .iter()
        .filter(|spec| spec.resume_supported)
        .collect()
}

#[must_use]
pub fn render_slash_command_help() -> String {
    let mut lines = vec![
        "Slash commands".to_string(),
        "  [resume] means the command also works with --resume SESSION.json".to_string(),
    ];
    for spec in slash_command_specs() {
        let name = match spec.argument_hint {
            Some(argument_hint) => format!("/{} {}", spec.name, argument_hint),
            None => format!("/{}", spec.name),
        };
        let resume = if spec.resume_supported {
            " [resume]"
        } else {
            ""
        };
        lines.push(format!("  {name:<20} {}{}", spec.summary, resume));
    }
    lines.join("\n")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlashCommandResult {
    pub message: String,
    pub session: Session,
}

#[must_use]
pub fn handle_slash_command(
    input: &str,
    session: &Session,
    compaction: CompactionConfig,
) -> Option<SlashCommandResult> {
    match SlashCommand::parse(input)? {
        SlashCommand::Compact => {
            let result = compact_session(session, compaction);
            let message = if result.removed_message_count == 0 {
                "Compaction skipped: session is below the compaction threshold.".to_string()
            } else {
                format!(
                    "Compacted {} messages into a resumable system summary.",
                    result.removed_message_count
                )
            };
            Some(SlashCommandResult {
                message,
                session: result.compacted_session,
            })
        }
        SlashCommand::Help => Some(SlashCommandResult {
            message: render_slash_command_help(),
            session: session.clone(),
        }),
        SlashCommand::Status
        | SlashCommand::Debug
        | SlashCommand::Model { .. }
        | SlashCommand::Clear
        | SlashCommand::Cost
        | SlashCommand::Config { .. }
        | SlashCommand::Version
        | SlashCommand::Export { .. }
        | SlashCommand::Sessions
        | SlashCommand::Auth { .. }
        | SlashCommand::Headed
        | SlashCommand::Headless
        | SlashCommand::Extension { .. }
        | SlashCommand::CloakBrowser
        | SlashCommand::Unknown(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        handle_slash_command, render_slash_command_help, resume_supported_slash_commands,
        slash_command_specs, SlashCommand,
    };
    use runtime::{CompactionConfig, ContentBlock, ConversationMessage, MessageRole, Session};

    #[test]
    #[allow(clippy::too_many_lines)]
    fn parses_supported_slash_commands() {
        assert_eq!(SlashCommand::parse("/help"), Some(SlashCommand::Help));
        assert_eq!(SlashCommand::parse(" /status "), Some(SlashCommand::Status));
        assert_eq!(SlashCommand::parse("/debug"), Some(SlashCommand::Debug));
        assert_eq!(
            SlashCommand::parse("/model anthropic/claude-opus-4-6"),
            Some(SlashCommand::Model {
                model: Some("anthropic/claude-opus-4-6".to_string()),
            })
        );
        assert_eq!(
            SlashCommand::parse("/model"),
            Some(SlashCommand::Model { model: None })
        );
        assert_eq!(SlashCommand::parse("/clear"), Some(SlashCommand::Clear));
        // /clear with any trailing args is rejected
        assert_eq!(
            SlashCommand::parse("/clear --confirm"),
            Some(SlashCommand::Unknown("clear".to_string()))
        );
        assert_eq!(SlashCommand::parse("/cost"), Some(SlashCommand::Cost));
        assert_eq!(
            SlashCommand::parse("/config"),
            Some(SlashCommand::Config { section: None })
        );
        assert_eq!(
            SlashCommand::parse("/config env"),
            Some(SlashCommand::Config {
                section: Some("env".to_string())
            })
        );
        assert_eq!(SlashCommand::parse("/version"), Some(SlashCommand::Version));
        assert_eq!(
            SlashCommand::parse("/export notes.txt"),
            Some(SlashCommand::Export {
                path: Some("notes.txt".to_string())
            })
        );
        assert_eq!(
            SlashCommand::parse("/sessions"),
            Some(SlashCommand::Sessions)
        );
        assert_eq!(
            SlashCommand::parse("/auth"),
            Some(SlashCommand::Auth { provider: None })
        );
        assert_eq!(
            SlashCommand::parse("/auth openai"),
            Some(SlashCommand::Auth {
                provider: Some("openai".to_string())
            })
        );
        assert_eq!(SlashCommand::parse("/headed"), Some(SlashCommand::Headed));
        assert_eq!(
            SlashCommand::parse("/headless"),
            Some(SlashCommand::Headless)
        );
        assert_eq!(
            SlashCommand::parse("/extension"),
            Some(SlashCommand::Extension { stop: false })
        );
        assert_eq!(
            SlashCommand::parse("/extension stop"),
            Some(SlashCommand::Extension { stop: true })
        );
        assert_eq!(
            SlashCommand::parse("/cloakbrowser"),
            Some(SlashCommand::CloakBrowser)
        );
    }

    #[test]
    fn parses_slash_commands_case_insensitively() {
        assert_eq!(SlashCommand::parse("/Help"), Some(SlashCommand::Help));
        assert_eq!(SlashCommand::parse("/STATUS"), Some(SlashCommand::Status));
        assert_eq!(SlashCommand::parse("/COMPACT"), Some(SlashCommand::Compact));
        assert_eq!(
            SlashCommand::parse("/Model anthropic/claude-opus-4-6"),
            Some(SlashCommand::Model {
                model: Some("anthropic/claude-opus-4-6".to_string()),
            })
        );
        assert_eq!(
            SlashCommand::parse("/Export Notes.txt"),
            Some(SlashCommand::Export {
                path: Some("Notes.txt".to_string()),
            })
        );
    }

    #[test]
    fn renders_help_from_shared_specs() {
        let help = render_slash_command_help();
        assert!(help.contains("works with --resume SESSION.json"));
        assert!(help.contains("/help"));
        assert!(help.contains("/status"));
        assert!(help.contains("/compact"));
        assert!(help.contains("/debug"));
        assert!(help.contains("/model [model]"));
        assert!(help.contains("/clear"));
        assert!(!help.contains("/clear [--confirm]"));
        assert!(help.contains("/cost"));
        assert!(help.contains("/config [model]"));
        assert!(help.contains("/version"));
        assert!(help.contains("/export [file]"));
        assert!(help.contains("/sessions"));
        assert!(help.contains("/auth [anthropic|openai|other]"));
        assert!(help.contains("/headed"));
        assert!(help.contains("/headless"));
        assert!(help.contains("/extension [stop]"));
        assert!(help.contains("/cloakbrowser"));
        assert!(!help.contains("/resume"));
        assert_eq!(slash_command_specs().len(), 17);
        assert_eq!(resume_supported_slash_commands().len(), 8);
    }

    #[test]
    fn compacts_sessions_via_slash_command() {
        // Plain user/assistant turns only — the compactor walks the preserved
        // window backwards across tool_use/tool_result pairs, so including a
        // Tool message would change the removal count and obscure what this
        // test is asserting (that /compact wires the count into the message).
        let session = Session {
            version: 1,
            model: None,
            title: None,
            messages: vec![
                ConversationMessage::user_text("a ".repeat(200)),
                ConversationMessage::assistant(vec![ContentBlock::Text {
                    text: "b ".repeat(200),
                }]),
                ConversationMessage::user_text("c ".repeat(200)),
                ConversationMessage::assistant(vec![ContentBlock::Text {
                    text: "recent".to_string(),
                }]),
            ],
            child_sessions: Vec::new(),
        };

        let result = handle_slash_command(
            "/compact",
            &session,
            CompactionConfig {
                preserve_recent_messages: 2,
                max_estimated_tokens: 1,
                ..CompactionConfig::default()
            },
        )
        .expect("slash command should be handled");

        assert!(result.message.contains("Compacted 2 messages"));
        assert_eq!(result.session.messages[0].role, MessageRole::System);
    }

    #[test]
    fn help_command_is_non_mutating() {
        let session = Session::new();
        let result = handle_slash_command("/help", &session, CompactionConfig::default())
            .expect("help command should be handled");
        assert_eq!(result.session, session);
        assert!(result.message.contains("Slash commands"));
    }

    #[test]
    fn ignores_unknown_or_runtime_bound_slash_commands() {
        let session = Session::new();
        assert!(handle_slash_command("/unknown", &session, CompactionConfig::default()).is_none());
        assert!(handle_slash_command("/status", &session, CompactionConfig::default()).is_none());
        assert!(handle_slash_command("/debug", &session, CompactionConfig::default()).is_none());
        assert!(
            handle_slash_command("/model claude", &session, CompactionConfig::default()).is_none()
        );
        assert!(handle_slash_command("/clear", &session, CompactionConfig::default()).is_none());
        assert!(handle_slash_command("/cost", &session, CompactionConfig::default()).is_none());
        assert!(handle_slash_command("/config", &session, CompactionConfig::default()).is_none());
        assert!(
            handle_slash_command("/config env", &session, CompactionConfig::default()).is_none()
        );
        assert!(handle_slash_command("/version", &session, CompactionConfig::default()).is_none());
        assert!(
            handle_slash_command("/export note.txt", &session, CompactionConfig::default())
                .is_none()
        );
        assert!(handle_slash_command("/sessions", &session, CompactionConfig::default()).is_none());
        assert!(
            handle_slash_command("/auth openai", &session, CompactionConfig::default()).is_none()
        );
        assert!(handle_slash_command("/headed", &session, CompactionConfig::default()).is_none());
        assert!(handle_slash_command("/headless", &session, CompactionConfig::default()).is_none());
        assert!(
            handle_slash_command("/extension", &session, CompactionConfig::default()).is_none()
        );
        assert!(
            handle_slash_command("/extension stop", &session, CompactionConfig::default())
                .is_none()
        );
        assert!(
            handle_slash_command("/cloakbrowser", &session, CompactionConfig::default()).is_none()
        );
    }
}
