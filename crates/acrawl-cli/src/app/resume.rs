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

    #[test]
    fn resume_command_outcome_holds_session_and_message() {
        let session = Session::new();
        let outcome = ResumeCommandOutcome {
            session: session.clone(),
            message: Some("test message".to_string()),
        };
        assert_eq!(outcome.message, Some("test message".to_string()));
        assert_eq!(outcome.session.messages.len(), 0);
    }

    #[test]
    fn resume_command_outcome_message_can_be_none() {
        let session = Session::new();
        let outcome = ResumeCommandOutcome {
            session,
            message: None,
        };
        assert_eq!(outcome.message, None);
    }
}
