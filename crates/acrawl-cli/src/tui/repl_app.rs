//! Ratatui REPL: header (model / session / cwd), transcript (top-anchored, wrapped), status, input.
//! Slash commands suspend the alternate screen and use the classic stdout path.

use std::io;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use ansi_to_tui::IntoText as _;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Margin};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, Borders, Clear, HighlightSpacing, List, ListItem, ListState, Paragraph, Wrap,
};
use ratatui::DefaultTerminal;
use runtime::{PermissionMode, PermissionPromptDecision, PermissionRequest};

use crate::app::{
    slash_command_completion_candidates, ChannelPermissionPrompter, LiveCli, AllowedToolSet,
};
use crate::tui::ReplTuiEvent;
use commands::SlashCommand;

/// One logical transcript row before width-aware wrapping for the List.
#[derive(Clone)]
enum TranscriptEntry {
    System(String),
    User(String),
    Stream(Line<'static>),
}

fn build_wrapped_list(entries: &[TranscriptEntry], width: u16) -> Vec<ListItem<'static>> {
    let w = usize::from(width.max(8));
    let mut out = Vec::new();
    let system_style = Style::default().fg(Color::DarkGray).italic();
    let you_style = Style::default().fg(Color::Cyan).bold();

    for e in entries {
        match e {
            TranscriptEntry::System(s) => {
                if s.is_empty() {
                    continue;
                }
                for line in textwrap::wrap(s.as_str(), w) {
                    out.push(ListItem::new(Line::from(vec![Span::styled(
                        line.into_owned(),
                        system_style,
                    )])));
                }
            }
            TranscriptEntry::User(s) => {
                let full = format!("You {s}");
                let rows: Vec<String> = textwrap::wrap(&full, w)
                    .into_iter()
                    .map(|c| c.into_owned())
                    .collect();
                for (i, row) in rows.into_iter().enumerate() {
                    if i == 0 && row.starts_with("You ") {
                        let rest = row.get(4..).unwrap_or("").to_string();
                        out.push(ListItem::new(Line::from(vec![
                            Span::styled("You ", you_style),
                            Span::raw(rest),
                        ])));
                    } else {
                        out.push(ListItem::new(Line::from(Span::raw(row))));
                    }
                }
            }
            TranscriptEntry::Stream(line) => {
                let s = line.to_string();
                let style = line.style;
                for cow in textwrap::wrap(&s, w) {
                    out.push(ListItem::new(Line::from(Span::styled(
                        cow.into_owned(),
                        style,
                    ))));
                }
            }
        }
    }
    out
}

enum WorkerMsg {
    RunTurn(String),
    Shutdown,
}

struct ReplTuiState {
    entries: Vec<TranscriptEntry>,
    list_state: ListState,
    input: String,
    status_line: String,
    busy: bool,
    pending_permission: Option<(PermissionRequest, Sender<PermissionPromptDecision>)>,
    exit: bool,
    persist_on_exit: bool,
    /// Caret visibility; toggled on a wall-clock deadline (not frame count).
    cursor_on: bool,
    cursor_blink_deadline: Instant,
}

impl ReplTuiState {
    fn new() -> Self {
        Self {
            entries: Vec::new(),
            list_state: ListState::default(),
            input: String::new(),
            status_line: String::new(),
            busy: false,
            pending_permission: None,
            exit: false,
            persist_on_exit: true,
            cursor_on: true,
            cursor_blink_deadline: Instant::now() + Duration::from_millis(530),
        }
    }

    fn tick_input_caret(&mut self) {
        if Instant::now() >= self.cursor_blink_deadline {
            self.cursor_on = !self.cursor_on;
            self.cursor_blink_deadline = Instant::now() + Duration::from_millis(530);
        }
    }

    fn wake_input_caret(&mut self) {
        self.cursor_on = true;
        self.cursor_blink_deadline = Instant::now() + Duration::from_millis(530);
    }

    fn push_user_line(&mut self, text: &str) {
        self.entries
            .push(TranscriptEntry::User(text.trim().to_string()));
    }

    fn push_stream_ansi(&mut self, ansi: &str) {
        let Ok(text) = ansi.as_bytes().into_text() else {
            return;
        };
        for ln in text.lines {
            self.entries.push(TranscriptEntry::Stream(ln));
        }
    }

    fn push_system(&mut self, msg: &str) {
        for row in msg.lines() {
            if row.is_empty() {
                self.entries.push(TranscriptEntry::System(" ".to_string()));
            } else {
                self.entries
                    .push(TranscriptEntry::System(row.to_string()));
            }
        }
    }

    fn drain_events(&mut self, rx: &Receiver<ReplTuiEvent>) {
        while let Ok(ev) = rx.try_recv() {
            match ev {
                ReplTuiEvent::StreamAnsi(s) => self.push_stream_ansi(&s),
                ReplTuiEvent::TurnStarting => {
                    self.busy = true;
                    // ASCII only: emoji width on ConPTY misaligns the rest of the status row (looks
                    // like an extra letter before "Ready").
                    self.status_line = "Thinking...".to_string();
                }
                ReplTuiEvent::TurnFinished(result) => {
                    self.busy = false;
                    self.status_line = match &result {
                        Ok(()) => "Ready".to_string(),
                        Err(e) => format!("Error: {e}"),
                    };
                    if let Err(e) = result {
                        self.push_system(&format!("Error: {e}"));
                    }
                }
                ReplTuiEvent::PermissionNeeded { request, respond } => {
                    self.pending_permission = Some((request, respond));
                }
                ReplTuiEvent::SystemMessage(s) => self.push_system(&s),
            }
        }
    }
}

fn header_lines(model: &str, perm: PermissionMode, session_id: &str, cwd: &str) -> Vec<Line<'static>> {
    let model = model.to_string();
    let session_id = session_id.to_string();
    let cwd = cwd.to_string();
    vec![
        Line::from(vec![
            Span::styled(
                " acrawl ",
                Style::default()
                    .fg(Color::Rgb(46, 160, 67))
                    .bold()
                    .bg(Color::Rgb(24, 24, 24)),
            ),
            Span::raw(" "),
            Span::styled(model, Style::default().fg(Color::LightCyan)),
        ]),
        Line::from(vec![
            Span::styled(" session ", Style::default().fg(Color::DarkGray)),
            Span::raw(session_id),
            Span::raw("  "),
            Span::styled(" perm ", Style::default().fg(Color::DarkGray)),
            Span::styled(perm.as_str(), Style::default().fg(Color::Yellow)),
        ]),
        Line::from(vec![
            Span::styled(" cwd ", Style::default().fg(Color::DarkGray)),
            Span::raw(cwd),
        ]),
    ]
}

fn draw_permission_modal(frame: &mut ratatui::Frame<'_>, request: &PermissionRequest) {
    let area = frame.area();
    let block_area = area.inner(Margin {
        horizontal: area.width / 6,
        vertical: area.height / 4,
    });
    frame.render_widget(Clear, block_area);
    let block = Block::default()
        .title(" Permission required ")
        .title_style(Style::default().fg(Color::Yellow).bold())
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Rgb(200, 120, 40)));
    let inner = block.inner(block_area);
    frame.render_widget(block, block_area);
    let text = Text::from(vec![
        Line::from(""),
        Line::from(vec![
            Span::styled("Tool: ", Style::default().fg(Color::DarkGray)),
            Span::styled(request.tool_name.clone(), Style::default().fg(Color::Cyan)),
        ]),
        Line::from(vec![
            Span::styled("Current mode: ", Style::default().fg(Color::DarkGray)),
            Span::raw(request.current_mode.as_str()),
        ]),
        Line::from(vec![
            Span::styled("Required mode: ", Style::default().fg(Color::DarkGray)),
            Span::styled(request.required_mode.as_str(), Style::default().fg(Color::LightRed)),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            request.input.clone(),
            Style::default().fg(Color::Gray),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("y", Style::default().fg(Color::Green).bold()),
            Span::raw(" allow   "),
            Span::styled("n", Style::default().fg(Color::Red).bold()),
            Span::raw(" deny"),
        ]),
    ]);
    let p = Paragraph::new(text).wrap(Wrap { trim: true });
    frame.render_widget(p, inner);
}

fn suspend_for_stdout(terminal: &mut DefaultTerminal, f: impl FnOnce()) -> io::Result<()> {
    ratatui::try_restore()?;
    f();
    *terminal = ratatui::try_init()?;
    Ok(())
}

/// Interactive REPL using Ratatui when stdout is a TTY (unless `ACRAWL_CLASSIC_REPL` is set).
pub fn run_repl_ratatui(
    model: String,
    allowed_tools: Option<AllowedToolSet>,
    permission_mode: PermissionMode,
) -> Result<(), Box<dyn std::error::Error>> {
    let (ui_tx, ui_rx) = mpsc::channel::<ReplTuiEvent>();
    let (work_tx, work_rx) = mpsc::channel::<WorkerMsg>();

    let cli = Arc::new(Mutex::new(LiveCli::new_with_ui_tx(
        model.clone(),
        true,
        allowed_tools,
        permission_mode,
        ui_tx.clone(),
    )?));

    let cli_w = Arc::clone(&cli);
    let ui_tx_w = ui_tx.clone();
    thread::spawn(move || {
        while let Ok(msg) = work_rx.recv() {
            match msg {
                WorkerMsg::RunTurn(line) => {
                    let mut g = cli_w.lock().expect("cli lock");
                    let perm = ChannelPermissionPrompter::new(ui_tx_w.clone());
                    let _ = g.run_turn_tui(&line, perm);
                }
                WorkerMsg::Shutdown => break,
            }
        }
    });

    let mut terminal = ratatui::init();
    let work_shutdown = work_tx.clone();
    let result = run_loop(
        &mut terminal,
        &ui_rx,
        work_tx,
        cli,
        model,
        permission_mode,
    );
    let _ = work_shutdown.send(WorkerMsg::Shutdown);
    ratatui::restore();
    result
}

#[allow(clippy::too_many_arguments)]
fn run_loop(
    terminal: &mut DefaultTerminal,
    ui_rx: &Receiver<ReplTuiEvent>,
    work_tx: Sender<WorkerMsg>,
    cli: Arc<Mutex<LiveCli>>,
    model: String,
    permission_mode: PermissionMode,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut state = ReplTuiState::new();
    {
        let g = cli.lock().expect("cli lock");
        let banner = g.startup_banner_plain();
        state.push_system(&banner);
    }

    loop {
        state.drain_events(ui_rx);

        if state.exit {
            if state.persist_on_exit {
                let g = cli.lock().expect("cli lock");
                g.persist_session()?;
            }
            break;
        }

        let g = cli.lock().expect("cli lock");
        let cwd = std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "<unknown>".into());
        let session_id = g.session_id().to_string();
        let hdr = header_lines(&model, permission_mode, &session_id, &cwd);
        drop(g);

        state.tick_input_caret();

        terminal.draw(|frame| {
            let area = frame.area();
            let chunks = Layout::default()
                .direction(ratatui::layout::Direction::Vertical)
                .constraints([
                    Constraint::Length(3),
                    Constraint::Min(4),
                    Constraint::Length(1),
                    Constraint::Length(3),
                ])
                .split(area);
            let header_a = chunks[0];
            let main_a = chunks[1];
            let status_a = chunks[2];
            let input_a = chunks[3];

            let header_block = Block::default()
                .borders(Borders::BOTTOM)
                .border_style(Style::default().fg(Color::DarkGray));
            let header_inner = header_block.inner(header_a);
            frame.render_widget(header_block, header_a);
            let header_par = Paragraph::new(Text::from(hdr));
            frame.render_widget(header_par, header_inner);

            let main_block = Block::default()
                .title(" Transcript ")
                .title_style(Style::default().fg(Color::Rgb(140, 180, 220)))
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Rgb(60, 80, 100)));
            let main_inner = main_block.inner(main_a);
            frame.render_widget(main_block, main_a);

            let wrapped = build_wrapped_list(&state.entries, main_inner.width);
            // Always anchor the transcript to the first wrapped row (no follow-bottom / wheel offset).
            state.list_state.select(None);

            let list = List::new(wrapped)
                .highlight_spacing(HighlightSpacing::Never)
                .scroll_padding(2);
            frame.render_stateful_widget(list, main_inner, &mut state.list_state);

            frame.render_widget(Clear, status_a);
            let status = Paragraph::new(Line::from(vec![Span::styled(
                if state.status_line.is_empty() {
                    "Ready — type a goal or /help"
                } else {
                    state.status_line.as_str()
                },
                Style::default().fg(Color::Rgb(180, 190, 200)),
            )]))
            .style(Style::default().bg(Color::Rgb(30, 30, 34)));
            frame.render_widget(status, status_a);

            let input_block = Block::default()
                .borders(Borders::TOP)
                .border_style(Style::default().fg(Color::DarkGray));
            let input_inner = input_block.inner(input_a);
            frame.render_widget(input_block, input_a);
            // ASCII prompt, default no-wrap Paragraph: avoids ConPTY width bugs with emoji/wrap.
            let caret = if state.busy {
                " "
            } else if state.cursor_on {
                "|"
            } else {
                " "
            };
            let input_line = Line::from(vec![
                Span::styled("> ", Style::default().fg(Color::DarkGray)),
                Span::raw(state.input.clone()),
                Span::styled(caret, Style::default().fg(Color::Rgb(220, 220, 230)).bold()),
            ]);
            frame.render_widget(Paragraph::new(input_line), input_inner);

            if let Some((ref req, _)) = state.pending_permission {
                draw_permission_modal(frame, req);
            }
        })?;

        if event::poll(Duration::from_millis(50))? {
            let ev = event::read()?;
            match ev {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                if let Some((req, respond)) = state.pending_permission.take() {
                    match key.code {
                        KeyCode::Char('y') | KeyCode::Char('Y') => {
                            let _ = respond.send(PermissionPromptDecision::Allow);
                        }
                        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                            let _ = respond.send(PermissionPromptDecision::Deny {
                                reason: format!(
                                    "tool '{}' denied from TUI permission dialog",
                                    req.tool_name
                                ),
                            });
                        }
                        _ => {
                            state.pending_permission = Some((req, respond));
                        }
                    }
                    continue;
                }

                match key.code {
                    KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        if state.busy {
                            continue;
                        }
                        state.exit = true;
                        state.persist_on_exit = true;
                    }
                    KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        if state.busy {
                            continue;
                        }
                        if state.input.is_empty() {
                            state.exit = true;
                            state.persist_on_exit = true;
                        }
                    }
                    KeyCode::Enter => {
                        if state.busy {
                            continue;
                        }
                        let line = std::mem::take(&mut state.input);
                        let trimmed = line.trim().to_string();
                        if trimmed.is_empty() {
                            state.wake_input_caret();
                            continue;
                        }
                        if matches!(trimmed.as_str(), "/exit" | "/quit") {
                            state.exit = true;
                            state.persist_on_exit = true;
                            continue;
                        }
                        if let Some(cmd) = SlashCommand::parse(&trimmed) {
                            suspend_for_stdout(terminal, || {
                                let mut g = cli.lock().expect("cli lock");
                                let _ = g.handle_repl_command(cmd);
                            })?;
                            state.push_system("(slash command — see output above)");
                            state.wake_input_caret();
                            continue;
                        }
                        state.push_user_line(&trimmed);
                        work_tx.send(WorkerMsg::RunTurn(trimmed))?;
                        state.wake_input_caret();
                    }
                    KeyCode::Char(c) => {
                        if state.busy {
                            continue;
                        }
                        state.input.push(c);
                        state.wake_input_caret();
                    }
                    KeyCode::Backspace => {
                        if state.input.pop().is_some() {
                            state.wake_input_caret();
                        }
                    }
                    KeyCode::Tab => {
                        if state.busy || !state.input.starts_with('/') {
                            continue;
                        }
                        let prefix = state.input.clone();
                        let candidates = slash_command_completion_candidates();
                        let matches: Vec<_> = candidates
                            .into_iter()
                            .filter(|c| c.starts_with(&prefix))
                            .collect();
                        if matches.len() == 1 {
                            state.input = matches[0].clone();
                            state.wake_input_caret();
                        }
                    }
                    _ => {}
                }
                }
                _ => {}
            }
        }
    }

    Ok(())
}
