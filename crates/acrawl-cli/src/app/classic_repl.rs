use crate::error::CliError;
use crate::input;
use commands::SlashCommand;

use super::{AllowedToolSet, LiveCli};

pub(crate) fn run_repl_classic(
    model: String,
    allowed_tools: Option<AllowedToolSet>,
) -> Result<(), CliError> {
    let mut cli = LiveCli::new(model, true, allowed_tools)?;
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
