use crate::error::CliError;
use crate::input;
use commands::SlashCommand;
use runtime::RuntimeObserver;
use std::io::Write;
use std::sync::Arc;

use super::{AllowedToolSet, LiveCli};

/// Observer for classic REPL that handles pause/resume via stdin.
struct ClassicReplObserver {
    control_state: Arc<runtime::ControlState>,
}

impl ClassicReplObserver {
    fn new(control_state: Arc<runtime::ControlState>) -> Self {
        Self { control_state }
    }
}

impl RuntimeObserver for ClassicReplObserver {
    fn on_pause_started(&mut self, reason: &str) {
        eprintln!("\n⏸ PAUSED: {reason}");
        eprintln!("Solve the problem in the browser, then press Enter to resume...");
        let _ = std::io::stderr().flush();

        let mut input = String::new();
        let _ = std::io::stdin().read_line(&mut input);

        self.control_state.resume();
    }

    fn on_pause_ended(&mut self) {
        eprintln!("✓ Resumed");
    }
}

pub(crate) fn run_repl_classic(
    model: String,
    allowed_tools: Option<AllowedToolSet>,
) -> Result<(), CliError> {
    let mut cli = LiveCli::new(model, true, allowed_tools)?;
    let control_state = cli.cancel_flag();
    cli.set_observer(Box::new(ClassicReplObserver::new(control_state)));

    let mut editor = input::LineEditor::new("> ", super::slash_command_completion_candidates());
    println!("{}", cli.startup_banner());

    loop {
        match editor.read_line()? {
            input::ReadOutcome::Submit(input) => {
                let trimmed = input.trim().to_string();
                if trimmed.is_empty() {
                    continue;
                }
                if trimmed.eq_ignore_ascii_case("/exit") || trimmed.eq_ignore_ascii_case("/quit") {
                    cli.persist_session()?;
                    break;
                }
                if let Some(command) = SlashCommand::parse(&trimmed) {
                    if cli.handle_repl_command(command)? {
                        cli.persist_session()?;
                    }
                    continue;
                }
                editor.push_history(input);
                cli.run_turn(&trimmed)?;
            }
            input::ReadOutcome::Cancel => {}
            input::ReadOutcome::Exit => {
                cli.persist_session()?;
                break;
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn slash_command_completion_candidates_includes_help() {
        let candidates = super::super::slash_command_completion_candidates();
        assert!(candidates.contains(&"/help".to_string()));
    }

    #[test]
    fn slash_command_completion_candidates_includes_status() {
        let candidates = super::super::slash_command_completion_candidates();
        assert!(candidates.contains(&"/status".to_string()));
    }
}
