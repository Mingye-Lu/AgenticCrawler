use std::fmt::Write as FmtWrite;
use std::io::{self, Write};

use crossterm::cursor::{MoveToColumn, RestorePosition, SavePosition};
use crossterm::style::{Color as CtColor, Print, ResetColor, SetForegroundColor};
use crossterm::terminal::{Clear, ClearType};
use crossterm::{execute, queue};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::struct_field_names)]
pub struct ColorTheme {
    pub spinner_active: CtColor,
    pub spinner_done: CtColor,
    pub spinner_failed: CtColor,
}

impl Default for ColorTheme {
    fn default() -> Self {
        Self {
            spinner_active: CtColor::Blue,
            spinner_done: CtColor::Green,
            spinner_failed: CtColor::Red,
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Spinner {
    frame_index: usize,
}

impl Spinner {
    const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn tick(
        &mut self,
        label: &str,
        theme: &ColorTheme,
        out: &mut impl Write,
    ) -> io::Result<()> {
        let frame = Self::FRAMES[self.frame_index % Self::FRAMES.len()];
        self.frame_index += 1;
        queue!(
            out,
            SavePosition,
            MoveToColumn(0),
            Clear(ClearType::CurrentLine),
            SetForegroundColor(theme.spinner_active),
            Print(format!("{frame} {label}")),
            ResetColor,
            RestorePosition
        )?;
        out.flush()
    }

    pub fn finish(
        &mut self,
        label: &str,
        theme: &ColorTheme,
        out: &mut impl Write,
    ) -> io::Result<()> {
        self.frame_index = 0;
        execute!(
            out,
            MoveToColumn(0),
            Clear(ClearType::CurrentLine),
            SetForegroundColor(theme.spinner_done),
            Print(format!("✔ {label}\n")),
            ResetColor
        )?;
        out.flush()
    }

    pub fn fail(
        &mut self,
        label: &str,
        theme: &ColorTheme,
        out: &mut impl Write,
    ) -> io::Result<()> {
        self.frame_index = 0;
        execute!(
            out,
            MoveToColumn(0),
            Clear(ClearType::CurrentLine),
            SetForegroundColor(theme.spinner_failed),
            Print(format!("✘ {label}\n")),
            ResetColor
        )?;
        out.flush()
    }
}

/// Zero-sized API-stability shim retained so external call sites
/// (`output_sink.rs`, `app/mod.rs`) compile without churn after the
/// `tui-markdown` swap. `markdown_to_ansi` no longer needs `&self`, and
/// `MarkdownStreamState::push`/`flush` still take `&TerminalRenderer` only
/// to preserve their signatures. `color_theme()` is the one method that
/// still carries useful state (the spinner palette).
#[derive(Debug, Default, Clone, Copy)]
pub struct TerminalRenderer {
    color_theme: ColorTheme,
}

impl TerminalRenderer {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn color_theme(&self) -> &ColorTheme {
        &self.color_theme
    }

    #[must_use]
    #[allow(clippy::unused_self)]
    pub fn markdown_to_ansi(&self, markdown: &str) -> String {
        text_to_ansi(&render_lines(markdown))
    }
}

#[must_use]
pub fn render_lines(markdown: &str) -> Vec<Line<'static>> {
    let text = tui_markdown::from_str(markdown);
    text.lines.into_iter().map(line_into_owned).collect()
}

fn line_into_owned(line: Line<'_>) -> Line<'static> {
    let style = line.style;
    let alignment = line.alignment;
    let spans: Vec<Span<'static>> = line
        .spans
        .into_iter()
        .map(|s| Span::styled(s.content.into_owned(), s.style))
        .collect();
    let mut owned = Line::from(spans).style(style);
    if let Some(a) = alignment {
        owned = owned.alignment(a);
    }
    owned
}

pub(crate) fn text_to_ansi(lines: &[Line<'_>]) -> String {
    let mut out = String::new();
    for (i, line) in lines.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let line_style = line.style;
        let mut prev_style: Option<Style> = None;
        let mut any_styled = false;
        for span in &line.spans {
            let style = line_style.patch(span.style);
            if prev_style != Some(style) {
                if any_styled {
                    out.push_str("\u{1b}[0m");
                }
                write_style(&mut out, style);
                prev_style = Some(style);
                any_styled = true;
            }
            out.push_str(&span.content);
        }
        if any_styled {
            out.push_str("\u{1b}[0m");
        }
    }
    out
}

fn write_style(out: &mut String, style: Style) {
    if let Some(c) = style.fg {
        write_color(out, c, true);
    }
    if let Some(c) = style.bg {
        write_color(out, c, false);
    }
    let m = style.add_modifier;
    if m.contains(Modifier::BOLD) {
        out.push_str("\u{1b}[1m");
    }
    if m.contains(Modifier::DIM) {
        out.push_str("\u{1b}[2m");
    }
    if m.contains(Modifier::ITALIC) {
        out.push_str("\u{1b}[3m");
    }
    if m.contains(Modifier::UNDERLINED) {
        out.push_str("\u{1b}[4m");
    }
    if m.contains(Modifier::CROSSED_OUT) {
        out.push_str("\u{1b}[9m");
    }
}

fn write_color(out: &mut String, c: Color, foreground: bool) {
    let (base, ext) = if foreground { (30, "38") } else { (40, "48") };
    match c {
        Color::Reset => {
            let _ = write!(out, "\u{1b}[{}m", base + 9);
        }
        Color::Black => {
            let _ = write!(out, "\u{1b}[{base}m");
        }
        Color::Red => {
            let _ = write!(out, "\u{1b}[{}m", base + 1);
        }
        Color::Green => {
            let _ = write!(out, "\u{1b}[{}m", base + 2);
        }
        Color::Yellow => {
            let _ = write!(out, "\u{1b}[{}m", base + 3);
        }
        Color::Blue => {
            let _ = write!(out, "\u{1b}[{}m", base + 4);
        }
        Color::Magenta => {
            let _ = write!(out, "\u{1b}[{}m", base + 5);
        }
        Color::Cyan => {
            let _ = write!(out, "\u{1b}[{}m", base + 6);
        }
        Color::Gray => {
            let _ = write!(out, "\u{1b}[{}m", base + 7);
        }
        Color::DarkGray => {
            let _ = write!(out, "\u{1b}[{}m", base + 60);
        }
        Color::LightRed => {
            let _ = write!(out, "\u{1b}[{}m", base + 61);
        }
        Color::LightGreen => {
            let _ = write!(out, "\u{1b}[{}m", base + 62);
        }
        Color::LightYellow => {
            let _ = write!(out, "\u{1b}[{}m", base + 63);
        }
        Color::LightBlue => {
            let _ = write!(out, "\u{1b}[{}m", base + 64);
        }
        Color::LightMagenta => {
            let _ = write!(out, "\u{1b}[{}m", base + 65);
        }
        Color::LightCyan => {
            let _ = write!(out, "\u{1b}[{}m", base + 66);
        }
        Color::White => {
            let _ = write!(out, "\u{1b}[{}m", base + 67);
        }
        Color::Rgb(r, g, b) => {
            let _ = write!(out, "\u{1b}[{ext};2;{r};{g};{b}m");
        }
        Color::Indexed(i) => {
            let _ = write!(out, "\u{1b}[{ext};5;{i}m");
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct MarkdownStreamState {
    pending: String,
}

impl MarkdownStreamState {
    #[must_use]
    pub fn push(&mut self, renderer: &TerminalRenderer, delta: &str) -> Option<String> {
        self.pending.push_str(delta);
        let split = find_stream_safe_boundary(&self.pending)?;
        let ready = self.pending[..split].to_string();
        self.pending.drain(..split);
        Some(renderer.markdown_to_ansi(&ready))
    }

    #[must_use]
    pub fn flush(&mut self, renderer: &TerminalRenderer) -> Option<String> {
        if self.pending.trim().is_empty() {
            self.pending.clear();
            None
        } else {
            let pending = std::mem::take(&mut self.pending);
            Some(renderer.markdown_to_ansi(&pending))
        }
    }
}

/// Drain everything up to the next stream-safe boundary in `buffer` and
/// render it through [`render_lines`]. Returns `None` if `buffer` has no
/// complete block yet (e.g. mid-paragraph or inside an open fence).
///
/// Used by the TUI typewriter so multi-line constructs (fenced code blocks,
/// tables, etc.) reach `tui_markdown::from_str` as a coherent chunk rather
/// than one orphan line at a time.
#[must_use]
pub fn drain_safe_boundary(buffer: &mut String) -> Option<Vec<Line<'static>>> {
    let split = find_stream_safe_boundary(buffer)?;
    let chunk: String = buffer.drain(..split).collect();
    Some(render_lines(&chunk))
}

fn find_stream_safe_boundary(markdown: &str) -> Option<usize> {
    let mut in_fence = false;
    let mut last_boundary = None;
    let mut pos = 0usize;

    for line in markdown.split_inclusive('\n') {
        let trimmed = line.trim_start();
        let opens_fence = trimmed.starts_with("```") || trimmed.starts_with("~~~");
        pos += line.len();

        if opens_fence {
            in_fence = !in_fence;
            if !in_fence {
                last_boundary = Some(pos);
            }
            continue;
        }

        if !in_fence && line.ends_with('\n') {
            last_boundary = Some(pos);
        }
    }

    last_boundary
}

pub fn strip_ansi(input: &str) -> String {
    let mut output = String::new();
    let mut chars = input.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for next in chars.by_ref() {
                    if next.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
        } else {
            output.push(ch);
        }
    }

    output
}

#[cfg(test)]
mod tests {
    use super::{
        drain_safe_boundary, render_lines, strip_ansi, text_to_ansi, MarkdownStreamState, Spinner,
        TerminalRenderer,
    };
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::text::{Line, Span};

    #[test]
    fn render_lines_styles_heading() {
        let lines = render_lines("# Heading\n");
        let plain = lines
            .iter()
            .flat_map(|line| line.spans.iter().map(|s| s.content.as_ref()))
            .collect::<String>();
        assert!(plain.contains("Heading"));
        // headings receive at least some style (color or bold)
        let any_styled = lines.iter().any(|line| {
            line.style.fg.is_some()
                || !line.style.add_modifier.is_empty()
                || line
                    .spans
                    .iter()
                    .any(|s| s.style.fg.is_some() || !s.style.add_modifier.is_empty())
        });
        assert!(any_styled, "heading should carry styling");
    }

    #[test]
    fn render_lines_handles_lists_and_emphasis() {
        let lines = render_lines("- one\n- two\n\n**bold** and *italic*\n");
        let plain = lines
            .iter()
            .flat_map(|line| line.spans.iter().map(|s| s.content.as_ref()))
            .collect::<String>();
        assert!(plain.contains("one"));
        assert!(plain.contains("two"));
        assert!(plain.contains("bold"));
        assert!(plain.contains("italic"));
    }

    #[test]
    fn markdown_to_ansi_round_trips_through_strip_ansi() {
        let renderer = TerminalRenderer::new();
        let ansi = renderer.markdown_to_ansi(
            "# Heading\n\n**bold** and `code` and a [link](https://example.com).\n",
        );
        let plain = strip_ansi(&ansi);
        for needle in ["Heading", "bold", "code", "link"] {
            assert!(plain.contains(needle), "missing {needle:?} in {plain:?}");
        }
        assert!(ansi.contains('\u{1b}'), "expected ANSI escapes");
    }

    #[test]
    fn markdown_to_ansi_handles_fenced_code_block() {
        let renderer = TerminalRenderer::new();
        let ansi = renderer.markdown_to_ansi("```rust\nfn hi() { println!(\"hi\"); }\n```\n");
        let plain = strip_ansi(&ansi);
        assert!(plain.contains("fn hi"));
    }

    #[test]
    fn empty_markdown_yields_empty_ansi() {
        let renderer = TerminalRenderer::new();
        let ansi = renderer.markdown_to_ansi("");
        assert!(ansi.is_empty(), "got {ansi:?}");
    }

    #[test]
    fn streaming_state_waits_for_complete_blocks() {
        let renderer = TerminalRenderer::new();
        let mut state = MarkdownStreamState::default();

        assert_eq!(state.push(&renderer, "# Heading"), None);
        let flushed = state
            .push(&renderer, "\n\nParagraph\n\n")
            .expect("completed block");
        let plain = strip_ansi(&flushed);
        assert!(plain.contains("Heading"));
        assert!(plain.contains("Paragraph"));

        assert_eq!(state.push(&renderer, "```rust\nfn main() {}\n"), None);
        let code = state
            .push(&renderer, "```\n")
            .expect("closed code fence flushes");
        assert!(strip_ansi(&code).contains("fn main()"));
    }

    #[test]
    fn streaming_state_buffers_tilde_fences() {
        let renderer = TerminalRenderer::new();
        let mut state = MarkdownStreamState::default();

        assert_eq!(state.push(&renderer, "~~~rust\nfn body() {}\n"), None);
        let code = state
            .push(&renderer, "~~~\n")
            .expect("closed tilde fence flushes");
        assert!(strip_ansi(&code).contains("fn body()"));
    }

    #[test]
    fn drain_safe_boundary_holds_open_fence() {
        let mut buffer = String::from("```rust\nfn pending() {}\n");
        assert!(drain_safe_boundary(&mut buffer).is_none());
        // buffer unchanged because no safe boundary reached
        assert!(buffer.starts_with("```rust"));

        buffer.push_str("```\n");
        let rendered = drain_safe_boundary(&mut buffer).expect("now boundary reached");
        let plain: String = rendered
            .iter()
            .flat_map(|line| line.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(plain.contains("fn pending"));
        assert!(
            buffer.is_empty(),
            "buffer should be drained, got {buffer:?}"
        );
    }

    #[test]
    fn text_to_ansi_emits_foreground_and_modifiers() {
        let style = Style::default()
            .fg(Color::Red)
            .add_modifier(Modifier::BOLD | Modifier::ITALIC);
        let line = Line::from(Span::styled("hi", style));
        let ansi = text_to_ansi(&[line]);
        assert!(ansi.contains("\u{1b}[31m"), "missing red fg in {ansi:?}");
        assert!(ansi.contains("\u{1b}[1m"), "missing bold in {ansi:?}");
        assert!(ansi.contains("\u{1b}[3m"), "missing italic in {ansi:?}");
        assert!(ansi.contains("hi"));
        assert!(ansi.ends_with("\u{1b}[0m"), "missing trailing reset");
    }

    #[test]
    fn text_to_ansi_emits_rgb_indexed_and_background() {
        let rgb = Style::default().fg(Color::Rgb(10, 20, 30));
        let indexed = Style::default().fg(Color::Indexed(208));
        let bg = Style::default().bg(Color::Black);
        let rgb_ansi = text_to_ansi(&[Line::from(Span::styled("a", rgb))]);
        let idx_ansi = text_to_ansi(&[Line::from(Span::styled("b", indexed))]);
        let bg_ansi = text_to_ansi(&[Line::from(Span::styled("c", bg))]);
        assert!(
            rgb_ansi.contains("\u{1b}[38;2;10;20;30m"),
            "got {rgb_ansi:?}"
        );
        assert!(idx_ansi.contains("\u{1b}[38;5;208m"), "got {idx_ansi:?}");
        assert!(bg_ansi.contains("\u{1b}[40m"), "got {bg_ansi:?}");
    }

    #[test]
    fn text_to_ansi_emits_one_reset_per_line_when_style_unchanged() {
        let style = Style::default().fg(Color::Green);
        let line = Line::from(vec![Span::styled("foo", style), Span::styled("bar", style)]);
        let ansi = text_to_ansi(&[line]);
        // Only one reset at end of line (style stayed the same across spans).
        let resets = ansi.matches("\u{1b}[0m").count();
        assert_eq!(resets, 1, "got {ansi:?}");
    }

    #[test]
    fn streaming_flush_drains_pending() {
        let renderer = TerminalRenderer::new();
        let mut state = MarkdownStreamState::default();
        assert_eq!(state.push(&renderer, "trailing text without newline"), None);
        let flushed = state.flush(&renderer).expect("flush emits remainder");
        assert!(strip_ansi(&flushed).contains("trailing text"));
        assert!(state.flush(&renderer).is_none());
    }

    #[test]
    fn strip_ansi_removes_sgr_sequences() {
        assert_eq!(strip_ansi("\u{1b}[31mred\u{1b}[0m"), "red");
        assert_eq!(strip_ansi("no escapes here"), "no escapes here");
    }

    #[test]
    fn spinner_advances_frames() {
        let renderer = TerminalRenderer::new();
        let mut spinner = Spinner::new();
        let mut out = Vec::new();
        spinner
            .tick("Working", renderer.color_theme(), &mut out)
            .expect("tick succeeds");
        spinner
            .tick("Working", renderer.color_theme(), &mut out)
            .expect("tick succeeds");

        let output = String::from_utf8_lossy(&out);
        assert!(output.contains("Working"));
    }
}
