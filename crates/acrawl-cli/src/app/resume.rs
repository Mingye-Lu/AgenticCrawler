use std::fs;
use std::path::Path;

use crate::error::CliError;
use crate::format::{
    format_compact_report, format_cost_report, format_status_report, render_config_report,
    render_export_text, render_repl_help, render_version_report, resolve_export_path,
    status_context, StatusUsage,
};
use commands::SlashCommand;
use runtime::{CompactionConfig, Session};

#[derive(Debug, Clone)]
pub(crate) struct ResumeCommandOutcome {
    pub(crate) session: Session,
    pub(crate) message: Option<String>,
}

pub(crate) fn run_resume_command(
    session_path: &Path,
    session: &Session,
    command: &SlashCommand,
) -> Result<ResumeCommandOutcome, CliError> {
    match command {
        SlashCommand::Help => Ok(ResumeCommandOutcome {
            session: session.clone(),
            message: Some(render_repl_help()),
        }),
        SlashCommand::Compact => {
            let result = runtime::compact_session(
                session,
                CompactionConfig {
                    max_estimated_tokens: 0,
                    ..CompactionConfig::default()
                },
            );
            let removed = result.removed_message_count;
            let kept = result.compacted_session.messages.len();
            let skipped = removed == 0;
            result.compacted_session.save_to_path(session_path)?;
            Ok(ResumeCommandOutcome {
                session: result.compacted_session,
                message: Some(format_compact_report(removed, kept, skipped)),
            })
        }
        SlashCommand::Clear { confirm } => {
            if !confirm {
                return Ok(ResumeCommandOutcome {
                    session: session.clone(),
                    message: Some(
                        "clear: confirmation required; rerun with /clear --confirm".to_string(),
                    ),
                });
            }
            let cleared = Session::new();
            cleared.save_to_path(session_path)?;
            Ok(ResumeCommandOutcome {
                session: cleared,
                message: Some(format!(
                    "Cleared resumed session file {}.",
                    session_path.display()
                )),
            })
        }
        SlashCommand::Status => {
            let tracker = runtime::UsageTracker::from_session(session);
            let usage = tracker.cumulative_usage();
            Ok(ResumeCommandOutcome {
                session: session.clone(),
                message: Some(format_status_report(
                    session.model.as_deref().unwrap_or("unknown"),
                    StatusUsage {
                        message_count: session.messages.len(),
                        turns: tracker.turns(),
                        latest: tracker.current_turn_usage(),
                        cumulative: usage,
                        estimated_tokens: 0,
                    },
                    &status_context(Some(session_path))?,
                )),
            })
        }
        SlashCommand::Cost => {
            let usage = runtime::UsageTracker::from_session(session).cumulative_usage();
            Ok(ResumeCommandOutcome {
                session: session.clone(),
                message: Some(format_cost_report(usage)),
            })
        }
        SlashCommand::Config { section } => Ok(ResumeCommandOutcome {
            session: session.clone(),
            message: Some(render_config_report(section.as_deref())?),
        }),
        SlashCommand::Version => Ok(ResumeCommandOutcome {
            session: session.clone(),
            message: Some(render_version_report()),
        }),
        SlashCommand::Export { path } => {
            let export_path = resolve_export_path(path.as_deref(), session)?;
            fs::write(&export_path, render_export_text(session))?;
            Ok(ResumeCommandOutcome {
                session: session.clone(),
                message: Some(format!(
                    "Export\n  Result           wrote transcript\n  File             {}\n  Messages         {}",
                    export_path.display(),
                    session.messages.len()
                )),
            })
        }
        SlashCommand::Debug
        | SlashCommand::Resume { .. }
        | SlashCommand::Model { .. }
        | SlashCommand::Session { .. }
        | SlashCommand::Auth { .. }
        | SlashCommand::Headed
        | SlashCommand::Headless
        | SlashCommand::Unknown(_) => Err("unsupported resumed slash command".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn resume_help_returns_help_text() {
        let session = Session::new();
        let path = PathBuf::from("/tmp/test-session.json");
        let outcome = run_resume_command(&path, &session, &SlashCommand::Help).unwrap();
        let msg = outcome.message.expect("help should produce output");
        assert!(msg.contains("help"), "help output should mention 'help'");
    }

    #[test]
    fn resume_version_returns_version() {
        let session = Session::new();
        let path = PathBuf::from("/tmp/test-session.json");
        let outcome = run_resume_command(&path, &session, &SlashCommand::Version).unwrap();
        let msg = outcome.message.expect("version should produce output");
        assert!(!msg.is_empty());
    }

    #[test]
    fn resume_unsupported_command_returns_error() {
        let session = Session::new();
        let path = PathBuf::from("/tmp/test-session.json");
        let result = run_resume_command(
            &path,
            &session,
            &SlashCommand::Model { model: Some("gpt-4o".to_string()) },
        );
        assert!(result.is_err());
    }

    #[test]
    fn resume_clear_without_confirm_warns() {
        let session = Session::new();
        let path = PathBuf::from("/tmp/test-session.json");
        let outcome = run_resume_command(&path, &session, &SlashCommand::Clear { confirm: false }).unwrap();
        let msg = outcome.message.expect("should produce warning");
        assert!(msg.contains("confirm"));
    }
}
