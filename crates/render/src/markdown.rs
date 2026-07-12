use std::borrow::Cow;
use std::fmt::Write as FmtWrite;
use std::io::{self, Write};
use std::iter::Peekable;
use std::str::Chars;

use crossterm::cursor::{MoveToColumn, RestorePosition, SavePosition};
use crossterm::style::{Color as CtColor, Print, ResetColor, SetForegroundColor};
use crossterm::terminal::{Clear, ClearType};
use crossterm::{execute, queue};
use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

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
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TABLES);

    // Single sanitization choke point: strip ANSI/C0 control sequences from
    // the raw markdown *before* it reaches the parser. Untrusted content
    // (e.g. scraped page text echoed back by the model) can carry raw ESC
    // bytes, and the markdown tokenizer can split an escape sequence across
    // multiple events (e.g. a stray `[` inside an OSC/CSI payload gets
    // tokenized as link syntax), so sanitizing per-`Event` after parsing is
    // not reliable. Scrubbing the source string first guarantees every
    // caller of `render_lines`/`markdown_to_ansi` is protected without an
    // opt-in per-caller `strip_ansi()` call.
    let sanitized = sanitize_untrusted_text(markdown);

    let mut writer = MdWriter::default();
    for event in Parser::new_ext(&sanitized, opts) {
        writer.handle_event(event);
    }
    writer.finish()
}

// Markdown renderer using pulldown-cmark directly.
// Architecture inspired by common patterns in the pulldown-cmark ecosystem
// (openai/codex Apache-2.0, nearai/ironclaw Apache-2.0, helix-editor MPL-2.0).

#[derive(Debug, Clone)]
struct IndentCtx {
    first: Vec<Span<'static>>,
    continuation: Vec<Span<'static>>,
    first_pending: bool,
}

#[derive(Debug, Default, Clone)]
struct TableState {
    alignments: Vec<Alignment>,
    header: Option<Vec<String>>,
    rows: Vec<Vec<String>>,
    current_row: Vec<String>,
    current_cell: String,
    in_head: bool,
}

#[derive(Debug, Default)]
struct MdWriter {
    lines: Vec<Line<'static>>,
    current_spans: Vec<Span<'static>>,
    inline_styles: Vec<Style>,
    list_indices: Vec<Option<u64>>,
    indent_stack: Vec<IndentCtx>,
    in_code_block: bool,
    code_block_buffer: String,
    needs_newline: bool,
    in_paragraph: bool,
    table_state: Option<TableState>,
    heading_level: Option<HeadingLevel>,
    blockquote_depth: usize,
}

impl MdWriter {
    fn handle_event(&mut self, event: Event<'_>) {
        if self.in_code_block {
            self.handle_code_block_event(event);
            return;
        }

        if self.handle_table_event(&event) {
            return;
        }

        match event {
            Event::Start(tag) => self.handle_start(tag),
            Event::End(tag) => self.handle_end(tag),
            Event::Text(text) | Event::InlineMath(text) | Event::DisplayMath(text) => {
                self.push_text(text.as_ref());
            }
            Event::Code(code) => {
                self.push_text_with_style(code.as_ref(), self.current_style().fg(Color::Cyan));
            }
            Event::Html(html) | Event::InlineHtml(html) => self.push_text(html.as_ref()),
            Event::SoftBreak => self.push_text(" "),
            Event::HardBreak => self.flush_current_line(false),
            Event::Rule => self.push_rule(),
            Event::TaskListMarker(done) => {
                self.push_text(if done { "[x] " } else { "[ ] " });
            }
            Event::FootnoteReference(label) => self.push_text(&format!("[{label}]")),
        }
    }

    fn handle_start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => {
                if !self.in_list() {
                    self.begin_block();
                }
                self.in_paragraph = true;
            }
            Tag::Heading { level, .. } => {
                self.begin_block();
                self.heading_level = Some(level);
                let style = heading_style(level);
                self.inline_styles.push(style);
                self.push_text_with_style(
                    &format!("{} ", "#".repeat(heading_level_number(level))),
                    style,
                );
            }
            Tag::BlockQuote(..) => {
                self.begin_block();
                self.blockquote_depth += 1;
            }
            Tag::CodeBlock(kind) => {
                match kind {
                    CodeBlockKind::Indented | CodeBlockKind::Fenced(_) => {}
                }
                self.begin_block();
                self.in_code_block = true;
                self.code_block_buffer.clear();
            }
            Tag::List(start) => {
                if self.current_spans.is_empty()
                    && self.needs_newline
                    && self.list_indices.is_empty()
                    && !self.in_paragraph
                {
                    self.push_blank_line();
                    self.needs_newline = false;
                } else if !self.current_spans.is_empty() {
                    self.flush_current_line(false);
                }
                self.list_indices.push(start);
            }
            Tag::Item => self.start_list_item(),
            Tag::Emphasis => self
                .inline_styles
                .push(Style::default().add_modifier(Modifier::ITALIC)),
            Tag::Strong => self
                .inline_styles
                .push(Style::default().add_modifier(Modifier::BOLD)),
            Tag::Strikethrough => {
                self.inline_styles
                    .push(Style::default().add_modifier(Modifier::CROSSED_OUT));
            }
            Tag::Link { .. } | Tag::Image { .. } => self.inline_styles.push(
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::UNDERLINED),
            ),
            Tag::Table(alignments) => {
                self.begin_block();
                self.table_state = Some(TableState {
                    alignments,
                    ..TableState::default()
                });
            }
            _ => {}
        }
    }

    fn handle_end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => {
                self.in_paragraph = false;
                self.flush_current_line(false);
                if !self.in_list() {
                    self.needs_newline = true;
                }
            }
            TagEnd::Heading(..) => {
                let _ = self.inline_styles.pop();
                let _ = self.heading_level.take();
                self.flush_current_line(false);
                self.needs_newline = true;
            }
            TagEnd::BlockQuote(..) => {
                self.flush_current_line(false);
                self.blockquote_depth = self.blockquote_depth.saturating_sub(1);
                self.needs_newline = true;
            }
            TagEnd::CodeBlock => {
                self.emit_code_block();
            }
            TagEnd::List(..) => {
                self.flush_current_line(false);
                self.list_indices.pop();
                if self.list_indices.is_empty() {
                    self.needs_newline = true;
                }
            }
            TagEnd::Item => self.finish_list_item(),
            TagEnd::Emphasis
            | TagEnd::Strong
            | TagEnd::Strikethrough
            | TagEnd::Link
            | TagEnd::Image => {
                let _ = self.inline_styles.pop();
            }
            TagEnd::Table => {
                self.render_table();
                self.needs_newline = true;
            }
            _ => {}
        }
    }

    fn handle_code_block_event(&mut self, event: Event<'_>) {
        match event {
            Event::End(TagEnd::CodeBlock) => self.emit_code_block(),
            Event::Text(text) | Event::Code(text) | Event::Html(text) | Event::InlineHtml(text) => {
                self.code_block_buffer.push_str(text.as_ref());
            }
            Event::SoftBreak | Event::HardBreak => self.code_block_buffer.push('\n'),
            _ => {}
        }
    }

    fn handle_table_event(&mut self, event: &Event<'_>) -> bool {
        let Some(state) = self.table_state.as_mut() else {
            return false;
        };

        match event {
            Event::Start(Tag::TableHead) => state.in_head = true,
            Event::End(TagEnd::TableHead) => state.in_head = false,
            Event::Start(Tag::TableRow) => state.current_row.clear(),
            Event::End(TagEnd::TableRow) => {
                let row = std::mem::take(&mut state.current_row);
                if state.in_head {
                    state.header = Some(row);
                } else {
                    state.rows.push(row);
                }
            }
            Event::Start(Tag::TableCell) => state.current_cell.clear(),
            Event::End(TagEnd::TableCell) => {
                state.current_row.push(normalize_cell(&state.current_cell));
                state.current_cell.clear();
            }
            Event::Text(text)
            | Event::Code(text)
            | Event::Html(text)
            | Event::InlineHtml(text)
            | Event::InlineMath(text)
            | Event::DisplayMath(text) => state.current_cell.push_str(text.as_ref()),
            Event::SoftBreak | Event::HardBreak => state.current_cell.push(' '),
            Event::End(TagEnd::Table) => {
                self.render_table();
                self.needs_newline = true;
            }
            _ => {}
        }

        true
    }

    fn begin_block(&mut self) {
        self.flush_current_line(false);
        if self.needs_newline {
            self.push_blank_line();
        }
        self.needs_newline = false;
    }

    fn push_blank_line(&mut self) {
        if self.lines.last().is_some_and(|line| !line.spans.is_empty()) {
            self.lines.push(Line::default());
        }
    }

    fn push_rule(&mut self) {
        self.begin_block();
        self.push_text_with_style(
            &"─".repeat(40),
            Style::default().add_modifier(Modifier::DIM),
        );
        self.flush_current_line(false);
        self.needs_newline = true;
    }

    fn current_style(&self) -> Style {
        self.inline_styles
            .iter()
            .copied()
            .fold(Style::default(), Style::patch)
    }

    fn push_text(&mut self, text: &str) {
        let sanitized = sanitize_untrusted_text(text);
        self.push_text_with_style(&sanitized, self.current_style());
    }

    fn push_text_with_style(&mut self, text: &str, style: Style) {
        if text.is_empty() {
            return;
        }
        self.ensure_line_prefix();
        self.current_spans
            .push(Span::styled(text.to_owned(), style));
    }

    fn ensure_line_prefix(&mut self) {
        if !self.current_spans.is_empty() {
            return;
        }

        for _ in 0..self.blockquote_depth {
            self.current_spans.push(Span::styled(
                "> ".to_owned(),
                Style::default().fg(Color::Green),
            ));
        }
        for ctx in &mut self.indent_stack {
            let spans = if ctx.first_pending {
                ctx.first_pending = false;
                ctx.first.clone()
            } else {
                ctx.continuation.clone()
            };
            self.current_spans.extend(spans);
        }
    }

    fn flush_current_line(&mut self, force_empty: bool) {
        if self.current_spans.is_empty() {
            if force_empty {
                self.lines.push(Line::default());
            }
            return;
        }

        self.lines
            .push(Line::from(std::mem::take(&mut self.current_spans)));
    }

    fn start_list_item(&mut self) {
        self.flush_current_line(false);
        let depth = self.list_indices.len();
        let base_indent = depth.saturating_sub(1) * 4;
        let marker = match self.list_indices.last().copied().flatten() {
            Some(index) => format!("{index}. "),
            None => "- ".to_owned(),
        };
        let marker_style = if self.list_indices.last().copied().flatten().is_some() {
            Style::default().fg(Color::LightBlue)
        } else {
            Style::default()
        };
        let mut first = Vec::new();
        if base_indent > 0 {
            first.push(Span::raw(" ".repeat(base_indent)));
        }
        first.push(Span::styled(marker.clone(), marker_style));
        let continuation = vec![Span::raw(" ".repeat(base_indent + marker.len()))];
        self.indent_stack.push(IndentCtx {
            first,
            continuation,
            first_pending: true,
        });
    }

    fn finish_list_item(&mut self) {
        self.flush_current_line(false);
        self.indent_stack.pop();
        if let Some(Some(index)) = self.list_indices.last_mut() {
            *index += 1;
        }
        self.in_paragraph = false;
        self.needs_newline = false;
    }

    fn emit_code_block(&mut self) {
        let code = std::mem::take(&mut self.code_block_buffer);
        self.in_code_block = false;
        let style = Style::default().fg(Color::Cyan);
        if code.is_empty() {
            self.flush_current_line(true);
            self.needs_newline = true;
            return;
        }

        for line in code.split_terminator('\n') {
            self.push_text_with_style(line, style);
            self.flush_current_line(false);
        }
        self.needs_newline = true;
    }

    fn render_table(&mut self) {
        let Some(state) = self.table_state.take() else {
            return;
        };

        let column_count = state
            .header
            .as_ref()
            .map_or(0, Vec::len)
            .max(state.rows.iter().map(Vec::len).max().unwrap_or(0))
            .max(state.alignments.len());
        if column_count == 0 {
            return;
        }

        let mut widths = vec![0usize; column_count];
        if let Some(header) = &state.header {
            for (idx, cell) in header.iter().enumerate() {
                widths[idx] = widths[idx].max(UnicodeWidthStr::width(cell.as_str()));
            }
        }
        for row in &state.rows {
            for (idx, cell) in row.iter().enumerate() {
                widths[idx] = widths[idx].max(UnicodeWidthStr::width(cell.as_str()));
            }
        }

        let border_style = Style::default().add_modifier(Modifier::DIM);
        self.lines
            .push(table_border_line(&widths, '┌', '┬', '┐', border_style));
        if let Some(header) = &state.header {
            self.lines.push(table_row_line(
                header,
                &widths,
                &state.alignments,
                border_style,
            ));
            self.lines
                .push(table_border_line(&widths, '├', '┼', '┤', border_style));
        }
        for row in &state.rows {
            self.lines.push(table_row_line(
                row,
                &widths,
                &state.alignments,
                border_style,
            ));
        }
        self.lines
            .push(table_border_line(&widths, '└', '┴', '┘', border_style));
    }

    fn finish(mut self) -> Vec<Line<'static>> {
        if self.in_code_block {
            self.emit_code_block();
        }
        self.flush_current_line(false);
        self.lines
    }

    fn in_list(&self) -> bool {
        !self.list_indices.is_empty()
    }
}

fn heading_style(level: HeadingLevel) -> Style {
    match level {
        HeadingLevel::H1 => Style::default().add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        HeadingLevel::H2 => Style::default().add_modifier(Modifier::BOLD),
        HeadingLevel::H3 => Style::default().add_modifier(Modifier::BOLD | Modifier::ITALIC),
        HeadingLevel::H4 | HeadingLevel::H5 | HeadingLevel::H6 => {
            Style::default().add_modifier(Modifier::ITALIC)
        }
    }
}

fn heading_level_number(level: HeadingLevel) -> usize {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

fn normalize_cell(cell: &str) -> String {
    cell.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn table_border_line(
    widths: &[usize],
    left: char,
    middle: char,
    right: char,
    style: Style,
) -> Line<'static> {
    let mut spans = vec![Span::styled(left.to_string(), style)];
    for (idx, width) in widths.iter().enumerate() {
        spans.push(Span::styled("─".repeat(*width + 2), style));
        spans.push(Span::styled(
            if idx + 1 == widths.len() {
                right
            } else {
                middle
            }
            .to_string(),
            style,
        ));
    }
    Line::from(spans)
}

fn table_row_line(
    row: &[String],
    widths: &[usize],
    alignments: &[Alignment],
    border_style: Style,
) -> Line<'static> {
    let mut spans = vec![Span::styled("│".to_owned(), border_style)];
    for (idx, width) in widths.iter().enumerate() {
        let cell = row.get(idx).map_or("", String::as_str);
        let alignment = alignments.get(idx).copied().unwrap_or(Alignment::None);
        spans.push(Span::raw(" ".to_owned()));
        spans.push(Span::raw(pad_cell(cell, *width, alignment)));
        spans.push(Span::raw(" ".to_owned()));
        spans.push(Span::styled("│".to_owned(), border_style));
    }
    Line::from(spans)
}

fn pad_cell(cell: &str, width: usize, alignment: Alignment) -> String {
    let cell_width = UnicodeWidthStr::width(cell);
    let padding = width.saturating_sub(cell_width);
    let (left, right) = match alignment {
        Alignment::Right => (padding, 0),
        Alignment::Center => (padding / 2, padding - (padding / 2)),
        Alignment::Left | Alignment::None => (0, padding),
    };
    format!("{}{}{}", " ".repeat(left), cell, " ".repeat(right))
}

#[must_use]
pub fn text_to_ansi(lines: &[Line<'_>]) -> String {
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
/// tables, etc.) reach [`render_lines`] as a coherent chunk rather than one
/// orphan line at a time.
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

#[must_use]
pub fn strip_ansi(input: &str) -> String {
    let mut output = String::new();
    let mut chars = input.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            skip_escape_sequence(&mut chars);
        } else {
            output.push(ch);
        }
    }

    output
}

/// Consume an ANSI escape sequence (everything after the leading `ESC`)
/// from `chars`, recognizing CSI (`ESC [ ... final-byte`), OSC/DCS/SOS/PM/APC
/// (`ESC ] | P | X | ^ | _ ... BEL-or-ST`), and generic two-character
/// escapes (`ESC` + one byte). This is shared by [`strip_ansi`] and
/// [`sanitize_untrusted_text`] so both stay in sync on what counts as a
/// "complete" escape sequence.
fn skip_escape_sequence(chars: &mut Peekable<Chars<'_>>) {
    match chars.peek() {
        Some('[') => {
            // CSI: consume parameter/intermediate bytes up to and including
            // the final byte (`@`..=`~`).
            chars.next();
            for next in chars.by_ref() {
                if ('\u{40}'..='\u{7e}').contains(&next) {
                    break;
                }
            }
        }
        Some(']' | 'P' | 'X' | '^' | '_') => {
            // OSC/DCS/SOS/PM/APC: consume until the string is terminated by
            // BEL (`\u{07}`) or ST (`ESC \`).
            chars.next();
            while let Some(next) = chars.next() {
                if next == '\u{07}' || next == '\u{9c}' {
                    break;
                }
                if next == '\u{1b}' && chars.peek() == Some(&'\\') {
                    chars.next();
                    break;
                }
            }
        }
        Some(_) => {
            // Generic two-byte escape (Fe/Fp/nF forms), e.g. `ESC c`, `ESC =`.
            chars.next();
        }
        None => {}
    }
}

/// Strip every C0 control character (except tab and newline, which are used
/// for formatting) and every ANSI escape sequence from `text`. This is the
/// single sanitization choke point applied to all text-bearing markdown
/// events before they become terminal output, so untrusted content (e.g.
/// scraped web text echoed back by the model) can never inject terminal
/// control/escape sequences.
fn sanitize_untrusted_text(input: &str) -> Cow<'_, str> {
    if !input
        .chars()
        .any(|ch| ch == '\u{1b}' || is_stripped_control(ch))
    {
        return Cow::Borrowed(input);
    }

    let mut output = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            skip_escape_sequence(&mut chars);
        } else if !is_stripped_control(ch) {
            output.push(ch);
        }
    }

    Cow::Owned(output)
}

/// True for C0 control characters (other than tab/newline) and the DEL/C1
/// control range, i.e. everything a well-behaved terminal writer should
/// never emit verbatim from untrusted input.
fn is_stripped_control(ch: char) -> bool {
    let c = ch as u32;
    (c < 0x20 && ch != '\t' && ch != '\n') || c == 0x7f || (0x80..=0x9f).contains(&c)
}

#[cfg(test)]
mod tests {
    use super::*;
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
    fn render_lines_nested_lists() {
        let md = "1. First item\n    - Nested bullet\n    - Another\n2. Second item\n";
        let lines = render_lines(md);
        let texts: Vec<String> = lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect()
            })
            .collect();
        let nested_line = texts
            .iter()
            .find(|text| text.contains("Nested bullet"))
            .expect("nested bullet line");
        let top_line = texts
            .iter()
            .find(|text| text.contains("First item"))
            .expect("first item line");
        let nested_indent = nested_line.len() - nested_line.trim_start().len();
        let top_indent = top_line.len() - top_line.trim_start().len();
        assert!(
            nested_indent > top_indent,
            "nested={nested_indent}, top={top_indent}, lines={texts:?}"
        );
    }

    #[test]
    fn render_lines_styled_list_items_same_line() {
        let md = "1. **Bold item** with text\n2. Normal item\n";
        let lines = render_lines(md);
        let texts: Vec<String> = lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect()
            })
            .filter(|text: &String| !text.is_empty())
            .collect();
        let first = texts
            .iter()
            .find(|text| text.contains("Bold item"))
            .expect("bold item line");
        assert!(
            first.contains("1.") || first.starts_with('1'),
            "marker and content on same line: {first:?}"
        );
    }

    #[test]
    fn render_lines_strips_injected_escape_sequences_from_text() {
        // Untrusted content (e.g. echoed scraped page text) can carry raw
        // ESC bytes. Every text-bearing event must be sanitized so these
        // never reach the terminal, regardless of which code path (plain
        // text, table cell, code block) the content flows through.
        let md = "Title: evil\u{1b}[31mRED\u{1b}]0;pwned\u{07} tail\n";
        let lines = render_lines(md);
        let plain: String = lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect();
        assert!(
            !plain.contains('\u{1b}'),
            "expected no raw ESC bytes in rendered spans: {plain:?}"
        );
        assert!(
            plain.contains("evilRED"),
            "text content preserved: {plain:?}"
        );
        assert!(plain.contains("tail"), "text content preserved: {plain:?}");
        assert!(
            !plain.contains("pwned"),
            "OSC payload should not leak into rendered text: {plain:?}"
        );

        let ansi = text_to_ansi(&lines);
        // Only legitimate styling escapes (from write_style) should be
        // present in the final ANSI string, never the raw bytes we fed in.
        assert!(
            !ansi.contains("\u{1b}]0;pwned"),
            "OSC injection must not survive into the ANSI stream: {ansi:?}"
        );
    }

    #[test]
    fn strip_ansi_removes_osc_sequences() {
        let input = "before\u{1b}]0;window-title\u{07}after";
        assert_eq!(strip_ansi(input), "beforeafter");
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
    fn render_lines_preserves_very_long_lines() {
        let long = "x".repeat(8_192);
        let lines = render_lines(&long);
        assert_eq!(lines.len(), 1, "expected a single rendered line");

        let plain: String = lines[0]
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
        assert_eq!(plain.len(), long.len());
        assert_eq!(plain, long);
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
    fn streaming_state_push_handles_incremental_content() {
        let rndr = TerminalRenderer::new();
        let mut state = MarkdownStreamState::default();

        assert_eq!(state.push(&rndr, "# He"), None);
        assert_eq!(state.push(&rndr, "ading"), None);
        let heading = state
            .push(&rndr, "\n\nPar")
            .expect("completed heading should flush before partial paragraph");
        assert!(strip_ansi(&heading).contains("Heading"), "got {heading:?}");

        let output = state
            .push(&rndr, "agraph with **bold** text\n")
            .expect("newline should flush a complete chunk");
        let plain = strip_ansi(&output);

        assert!(plain.contains("Paragraph with bold text"), "got {plain:?}");
        assert!(
            output.contains("\u{1b}[1m"),
            "expected bold styling in {output:?}"
        );
        assert!(
            state.flush(&rndr).is_none(),
            "no trailing content should remain"
        );
    }

    #[test]
    fn streaming_state_holds_partial_content_until_boundary() {
        let rndr = TerminalRenderer::new();
        let mut state = MarkdownStreamState::default();

        assert_eq!(state.push(&rndr, "partial paragraph"), None);
        assert_eq!(state.push(&rndr, " still pending"), None);

        let flushed = state.flush(&rndr).expect("flush should emit pending text");
        assert_eq!(strip_ansi(&flushed), "partial paragraph still pending");
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

    fn md_to_ansi(md: &str) -> String {
        let renderer = TerminalRenderer::new();
        renderer.markdown_to_ansi(md)
    }

    #[test]
    fn golden_h1_bold_underlined() {
        let ansi = md_to_ansi("# Hello\n");
        // H1 style: BOLD + UNDERLINED
        assert!(
            ansi.contains("\x1b[1m"),
            "H1 must contain BOLD escape, got: {ansi:?}"
        );
        assert!(
            ansi.contains("\x1b[4m"),
            "H1 must contain UNDERLINED escape, got: {ansi:?}"
        );
        // Heading prefix preserved in output
        assert!(
            ansi.contains("# Hello"),
            "H1 must contain '# Hello', got: {ansi:?}"
        );
        // Exact golden output: styled prefix + text, single reset
        assert_eq!(ansi, "\x1b[1m\x1b[4m# Hello\x1b[0m");
    }

    #[test]
    fn golden_h2_bold_only() {
        let ansi = md_to_ansi("## World\n");
        // H2 style: BOLD only (no underline, no italic)
        assert!(
            ansi.contains("\x1b[1m"),
            "H2 must contain BOLD, got: {ansi:?}"
        );
        assert!(
            !ansi.contains("\x1b[4m"),
            "H2 must NOT contain UNDERLINED, got: {ansi:?}"
        );
        assert!(
            !ansi.contains("\x1b[3m"),
            "H2 must NOT contain ITALIC, got: {ansi:?}"
        );
        assert_eq!(ansi, "\x1b[1m## World\x1b[0m");
    }

    #[test]
    fn golden_h3_bold_italic() {
        let ansi = md_to_ansi("### Third\n");
        // H3 style: BOLD + ITALIC
        assert!(
            ansi.contains("\x1b[1m"),
            "H3 must contain BOLD, got: {ansi:?}"
        );
        assert!(
            ansi.contains("\x1b[3m"),
            "H3 must contain ITALIC, got: {ansi:?}"
        );
        assert!(
            !ansi.contains("\x1b[4m"),
            "H3 must NOT contain UNDERLINED, got: {ansi:?}"
        );
        assert_eq!(ansi, "\x1b[1m\x1b[3m### Third\x1b[0m");
    }

    #[test]
    fn golden_bold_text() {
        let ansi = md_to_ansi("**bold**\n");
        // Bold text uses BOLD modifier
        assert!(
            ansi.contains("\x1b[1m"),
            "bold must contain BOLD escape, got: {ansi:?}"
        );
        assert!(
            ansi.contains("bold"),
            "must contain text 'bold', got: {ansi:?}"
        );
        assert_eq!(ansi, "\x1b[1mbold\x1b[0m");
    }

    #[test]
    fn golden_italic_text() {
        let ansi = md_to_ansi("*italic*\n");
        // Italic text uses ITALIC modifier
        assert!(
            ansi.contains("\x1b[3m"),
            "italic must contain ITALIC escape, got: {ansi:?}"
        );
        assert!(
            !ansi.contains("\x1b[1m"),
            "italic must NOT contain BOLD, got: {ansi:?}"
        );
        assert_eq!(ansi, "\x1b[3mitalic\x1b[0m");
    }

    #[test]
    fn golden_inline_code_cyan() {
        let ansi = md_to_ansi("`code`\n");
        // Inline code renders with Cyan foreground (color code 36)
        assert!(
            ansi.contains("\x1b[36m"),
            "inline code must contain Cyan fg \\x1b[36m, got: {ansi:?}"
        );
        assert!(ansi.contains("code"), "must contain text, got: {ansi:?}");
        assert_eq!(ansi, "\x1b[36mcode\x1b[0m");
    }

    #[test]
    fn golden_fenced_code_block_cyan() {
        let ansi = md_to_ansi("```\nhello world\n```\n");
        // Fenced code blocks render with Cyan foreground
        assert!(
            ansi.contains("\x1b[36m"),
            "fenced code must contain Cyan fg, got: {ansi:?}"
        );
        assert!(
            ansi.contains("hello world"),
            "must contain code text, got: {ansi:?}"
        );
        assert_eq!(ansi, "\x1b[36mhello world\x1b[0m");
    }

    #[test]
    fn golden_fenced_code_block_with_language() {
        let ansi = md_to_ansi("```rust\nfn main() {}\n```\n");
        // Language-tagged fenced blocks also render Cyan
        assert!(
            ansi.contains("\x1b[36m"),
            "tagged code must contain Cyan, got: {ansi:?}"
        );
        assert!(
            ansi.contains("fn main() {}"),
            "must preserve code content, got: {ansi:?}"
        );
        assert_eq!(ansi, "\x1b[36mfn main() {}\x1b[0m");
    }

    #[test]
    fn golden_link_cyan_underlined() {
        let ansi = md_to_ansi("[click here](https://example.com)\n");
        // Links render with Cyan fg + UNDERLINED
        assert!(
            ansi.contains("\x1b[36m"),
            "link must contain Cyan fg, got: {ansi:?}"
        );
        assert!(
            ansi.contains("\x1b[4m"),
            "link must contain UNDERLINED, got: {ansi:?}"
        );
        assert!(
            ansi.contains("click here"),
            "link text must be present, got: {ansi:?}"
        );
        assert_eq!(ansi, "\x1b[36m\x1b[4mclick here\x1b[0m");
    }

    #[test]
    fn golden_unordered_list() {
        let ansi = md_to_ansi("- one\n- two\n");
        let plain = strip_ansi(&ansi);
        // Unordered list uses "- " prefix with default style
        assert!(
            plain.contains("- one"),
            "must contain '- one', got: {plain:?}"
        );
        assert!(
            plain.contains("- two"),
            "must contain '- two', got: {plain:?}"
        );
        // Markers have no color styling (default style emits trailing reset only)
        assert!(
            !ansi.contains("\x1b[94m"),
            "unordered list must NOT have LightBlue markers, got: {ansi:?}"
        );
    }

    #[test]
    fn golden_ordered_list_lightblue_markers() {
        let ansi = md_to_ansi("1. first\n2. second\n");
        // Ordered list markers are colored LightBlue (ANSI code 94)
        assert!(
            ansi.contains("\x1b[94m"),
            "ordered list must use LightBlue (94) for markers, got: {ansi:?}"
        );
        // Verify marker text
        assert!(
            ansi.contains("1. "),
            "must contain '1. ' marker, got: {ansi:?}"
        );
        assert!(
            ansi.contains("2. "),
            "must contain '2. ' marker, got: {ansi:?}"
        );
        // Content text follows after reset
        let plain = strip_ansi(&ansi);
        assert!(plain.contains("first"), "got: {plain:?}");
        assert!(plain.contains("second"), "got: {plain:?}");
    }

    #[test]
    fn golden_table_dim_borders() {
        let ansi = md_to_ansi("text\n\n| A | B |\n|---|---|\n| 1 | 2 |\n");
        // Table borders use DIM modifier (ANSI code 2)
        assert!(
            ansi.contains("\x1b[2m"),
            "table must use DIM for borders, got: {ansi:?}"
        );
        // Box-drawing characters present
        assert!(
            ansi.contains('┌'),
            "table must have top-left corner, got: {ansi:?}"
        );
        assert!(
            ansi.contains('┘'),
            "table must have bottom-right corner, got: {ansi:?}"
        );
        assert!(
            ansi.contains('│'),
            "table must have vertical borders, got: {ansi:?}"
        );
        assert!(
            ansi.contains('─'),
            "table must have horizontal borders, got: {ansi:?}"
        );
        // Cell content preserved
        let plain = strip_ansi(&ansi);
        assert!(plain.contains('1'), "cell 1 missing, got: {plain:?}");
        assert!(plain.contains('2'), "cell 2 missing, got: {plain:?}");
    }

    #[test]
    fn golden_nested_bold_in_italic() {
        let ansi = md_to_ansi("*italic **and bold***\n");
        // Contains ITALIC for outer
        assert!(
            ansi.contains("\x1b[3m"),
            "must contain ITALIC, got: {ansi:?}"
        );
        // Contains BOLD for nested
        assert!(
            ansi.contains("\x1b[1m"),
            "must contain BOLD for nested, got: {ansi:?}"
        );
        // All text present
        let plain = strip_ansi(&ansi);
        assert!(plain.contains("italic"), "got: {plain:?}");
        assert!(plain.contains("and bold"), "got: {plain:?}");
    }

    #[test]
    fn golden_blockquote_green_prefix() {
        let ansi = md_to_ansi("> quoted text\n");
        // Block quotes use Green foreground (ANSI 32) for the "> " prefix
        assert!(
            ansi.contains("\x1b[32m"),
            "blockquote prefix must be Green (32), got: {ansi:?}"
        );
        let plain = strip_ansi(&ansi);
        assert!(
            plain.contains("> "),
            "must show '> ' prefix, got: {plain:?}"
        );
        assert!(
            plain.contains("quoted text"),
            "must contain text, got: {plain:?}"
        );
    }

    #[test]
    fn golden_horizontal_rule_dim() {
        let ansi = md_to_ansi("---\n");
        // Horizontal rules render as repeated "─" with DIM
        assert!(ansi.contains("\x1b[2m"), "rule must use DIM, got: {ansi:?}");
        assert!(
            ansi.contains("─"),
            "rule must use box-drawing dash, got: {ansi:?}"
        );
        // Rule is 40 chars wide
        let plain = strip_ansi(&ansi);
        let dashes = plain.chars().filter(|&c| c == '─').count();
        assert_eq!(dashes, 40, "rule must be 40 dashes, got {dashes}");
    }

    #[test]
    fn golden_strikethrough() {
        let ansi = md_to_ansi("~~struck~~\n");
        // Strikethrough uses CROSSED_OUT (ANSI 9)
        assert!(
            ansi.contains("\x1b[9m"),
            "strikethrough must use CROSSED_OUT (9), got: {ansi:?}"
        );
        assert!(
            ansi.contains("struck"),
            "text must be present, got: {ansi:?}"
        );
        assert_eq!(ansi, "\x1b[9mstruck\x1b[0m");
    }

    #[test]
    fn golden_mixed_paragraph_resets_between_styles() {
        let ansi = md_to_ansi("normal **bold** normal\n");
        // After bold text, reset is emitted before returning to normal
        assert!(
            ansi.contains("\x1b[0m"),
            "must contain resets, got: {ansi:?}"
        );
        assert!(
            ansi.contains("\x1b[1m"),
            "bold section present, got: {ansi:?}"
        );
        // Structure: unstyled "normal " → BOLD "bold" → reset → unstyled " normal" → reset
        let plain = strip_ansi(&ansi);
        assert_eq!(plain, "normal bold normal");
    }
}
