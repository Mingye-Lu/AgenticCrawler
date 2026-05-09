use crate::error::CliError;
use crate::input;
use commands::SlashCommand;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use super::{AllowedToolSet, LiveCli};

/// Spawn a background thread that polls `ControlState` for pause events.
///
/// This mirrors the TUI render loop's polling approach: the TUI detects pause
/// by checking `cancel_flag.is_paused()` each frame. In classic REPL, since the
/// main thread is blocked on the runtime future, this thread handles the
/// stdin-based resume interaction.
fn spawn_pause_monitor(
    control_state: Arc<runtime::ControlState>,
    stop: Arc<AtomicBool>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        while !stop.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(100));

            if control_state.is_paused() {
                let reason = control_state.pause_reason();
                let display_reason = if reason.is_empty() {
                    "Human intervention requested".to_string()
                } else {
                    reason
                };
                eprintln!("\n-- PAUSED: {display_reason} --");
                eprintln!("Solve the problem in the browser, then press Enter to resume...");
                let _ = std::io::stderr().flush();

                let mut buf = String::new();
                let _ = std::io::stdin().read_line(&mut buf);

                if control_state.is_paused() {
                    control_state.resume();
                }
                eprintln!("Resuming...");
            }
        }
    })
}

pub(crate) fn run_repl_classic(
    model: String,
    allowed_tools: Option<AllowedToolSet>,
) -> Result<(), CliError> {
    let mut cli = LiveCli::new(model, true, allowed_tools)?;
    let control_state = cli.cancel_flag();

    let monitor_stop = Arc::new(AtomicBool::new(false));
    let monitor_handle = spawn_pause_monitor(control_state, Arc::clone(&monitor_stop));

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

    monitor_stop.store(true, Ordering::Relaxed);
    let _ = monitor_handle.join();

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
