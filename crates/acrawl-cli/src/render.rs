use std::fmt::Write as FmtWrite;
use std::io::{self, Write};

use crossterm::cursor::{MoveToColumn, RestorePosition, SavePosition};
use crossterm::style::{Color, Print, ResetColor, SetForegroundColor, Stylize};
use crossterm::terminal::{Clear, ClearType};
use crossterm::{execute, queue};
use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use syntect::easy::HighlightLines;
use syntect::highlighting::{Theme, ThemeSet};
use syntect::parsing::SyntaxSet;
use syntect::util::{as_24_bit_terminal_escaped, LinesWithEndings};

use crate::display_width::text_display_width;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColorTheme {
    heading: Color,
    emphasis: Color,
    strong: Color,
    inline_code: Color,
    link: Color,
    quote: Color,
    table_border: Color,
    code_block_border: Color,
    spinner_active: Color,
    spinner_done: Color,
    spinner_failed: Color,
}

impl Default for ColorTheme {
    fn default() -> Self {
        Self {
            heading: Color::Cyan,
            emphasis: Color::Magenta,
            strong: Color::Yellow,
            inline_code: Color::Green,
            link: Color::Blue,
            quote: Color::DarkGrey,
            table_border: Color::DarkCyan,
            code_block_border: Color::DarkGrey,
            spinner_active: Color::Blue,
            spinner_done: Color::Green,
            spinner_failed: Color::Red,
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

#[derive(Debug, Clone, PartialEq, Eq)]
enum ListKind {
    Unordered,
    Ordered { next_index: u64 },
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct TableState {
    headers: Vec<String>,
    rows: Vec<Vec<String>>,
    current_row: Vec<String>,
    current_cell: String,
    in_head: bool,
}

impl TableState {
    fn push_cell(&mut self) {
        let cell = self.current_cell.trim().to_string();
        self.current_row.push(cell);
        self.current_cell.clear();
    }

    fn finish_row(&mut self) {
        if self.current_row.is_empty() {
            return;
        }
        let row = std::mem::take(&mut self.current_row);
        if self.in_head {
            self.headers = row;
        } else {
            self.rows.push(row);
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct RenderState {
    emphasis: usize,
    strong: usize,
    heading_level: Option<u8>,
    quote: usize,
    list_stack: Vec<ListKind>,
    link_stack: Vec<LinkState>,
    table: Option<TableState>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LinkState {
    destination: String,
    text: String,
}

impl RenderState {
    fn style_text(&self, text: &str, theme: &ColorTheme) -> String {
        let mut style = text.stylize();

        if matches!(self.heading_level, Some(1 | 2)) || self.strong > 0 {
            style = style.bold();
        }
        if self.emphasis > 0 {
            style = style.italic();
        }

        if let Some(level) = self.heading_level {
            style = match level {
                1 => style.with(theme.heading),
                2 => style.white(),
                3 => style.with(Color::Blue),
                _ => style.with(Color::Grey),
            };
        } else if self.strong > 0 {
            style = style.with(theme.strong);
        } else if self.emphasis > 0 {
            style = style.with(theme.emphasis);
        }

        if self.quote > 0 {
            style = style.with(theme.quote);
        }

        format!("{style}")
    }

    fn append_raw(&mut self, output: &mut String, text: &str) {
        if let Some(link) = self.link_stack.last_mut() {
            link.text.push_str(text);
        } else if let Some(table) = self.table.as_mut() {
            table.current_cell.push_str(text);
        } else {
            output.push_str(text);
        }
    }

    fn append_styled(&mut self, output: &mut String, text: &str, theme: &ColorTheme) {
        let styled = self.style_text(text, theme);
        self.append_raw(output, &styled);
    }
}

#[derive(Debug)]
pub struct TerminalRenderer {
    syntax_set: SyntaxSet,
    syntax_theme: Theme,
    color_theme: ColorTheme,
}

impl Default for TerminalRenderer {
    fn default() -> Self {
        let syntax_set = SyntaxSet::load_defaults_newlines();
        let syntax_theme = ThemeSet::load_defaults()
            .themes
            .remove("base16-ocean.dark")
            .unwrap_or_default();
        Self {
            syntax_set,
            syntax_theme,
            color_theme: ColorTheme::default(),
        }
    }
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
    pub fn render_markdown(&self, markdown: &str) -> String {
        let mut output = String::new();
        let mut state = RenderState::default();
        let mut code_language = String::new();
        let mut code_buffer = String::new();
        let mut in_code_block = false;

        for event in Parser::new_ext(markdown, Options::all()) {
            self.render_event(
                event,
                &mut state,
                &mut output,
                &mut code_buffer,
                &mut code_language,
                &mut in_code_block,
            );
        }

        output.clone()
    }

    #[must_use]
    pub fn markdown_to_ansi(&self, markdown: &str) -> String {
        self.render_markdown(markdown)
    }

    #[allow(clippy::too_many_lines)]
    fn render_event(
        &self,
        event: Event<'_>,
        state: &mut RenderState,
        output: &mut String,
        code_buffer: &mut String,
        code_language: &mut String,
        in_code_block: &mut bool,
    ) {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                self.start_heading(state, level as u8, output);
            }
            Event::End(TagEnd::Paragraph) => output.push_str("\n\n"),
            Event::Start(Tag::BlockQuote(..)) => self.start_quote(state, output),
            Event::End(TagEnd::BlockQuote(..)) => {
                state.quote = state.quote.saturating_sub(1);
                output.push('\n');
            }
            Event::End(TagEnd::Heading(..)) => {
                state.heading_level = None;
                output.push_str("\n\n");
            }
            Event::End(TagEnd::Item) | Event::SoftBreak | Event::HardBreak => {
                state.append_raw(output, "\n");
            }
            Event::Start(Tag::List(first_item)) => {
                let kind = match first_item {
                    Some(index) => ListKind::Ordered { next_index: index },
                    None => ListKind::Unordered,
                };
                state.list_stack.push(kind);
            }
            Event::End(TagEnd::List(..)) => {
                state.list_stack.pop();
                output.push('\n');
            }
            Event::Start(Tag::Item) => Self::start_item(state, output),
            Event::Start(Tag::CodeBlock(kind)) => {
                *in_code_block = true;
                *code_language = match kind {
                    CodeBlockKind::Indented => String::from("text"),
                    CodeBlockKind::Fenced(lang) => lang.to_string(),
                };
                code_buffer.clear();
                self.start_code_block(code_language, output);
            }
            Event::End(TagEnd::CodeBlock) => {
                self.finish_code_block(code_buffer, code_language, output);
                *in_code_block = false;
                code_language.clear();
                code_buffer.clear();
            }
            Event::Start(Tag::Emphasis) => state.emphasis += 1,
            Event::End(TagEnd::Emphasis) => state.emphasis = state.emphasis.saturating_sub(1),
            Event::Start(Tag::Strong) => state.strong += 1,
            Event::End(TagEnd::Strong) => state.strong = state.strong.saturating_sub(1),
            Event::Code(code) => {
                let rendered =
                    format!("{}", format!("`{code}`").with(self.color_theme.inline_code));
                state.append_raw(output, &rendered);
            }
            Event::Rule => output.push_str("---\n"),
            Event::Text(text) => {
                self.push_text(text.as_ref(), state, output, code_buffer, *in_code_block);
            }
            Event::Html(html) | Event::InlineHtml(html) => {
                state.append_raw(output, &html);
            }
            Event::FootnoteReference(reference) => {
                state.append_raw(output, &format!("[{reference}]"));
            }
            Event::TaskListMarker(done) => {
                state.append_raw(output, if done { "[x] " } else { "[ ] " });
            }
            Event::InlineMath(math) | Event::DisplayMath(math) => {
                state.append_raw(output, &math);
            }
            Event::Start(Tag::Link { dest_url, .. }) => {
                state.link_stack.push(LinkState {
                    destination: dest_url.to_string(),
                    text: String::new(),
                });
            }
            Event::End(TagEnd::Link) => {
                if let Some(link) = state.link_stack.pop() {
                    let label = if link.text.is_empty() {
                        link.destination.clone()
                    } else {
                        link.text
                    };
                    let rendered = format!(
                        "{}",
                        format!("[{label}]({})", link.destination)
                            .underlined()
                            .with(self.color_theme.link)
                    );
                    state.append_raw(output, &rendered);
                }
            }
            Event::Start(Tag::Image { dest_url, .. }) => {
                let rendered = format!(
                    "{}",
                    format!("[image:{dest_url}]").with(self.color_theme.link)
                );
                state.append_raw(output, &rendered);
            }
            Event::Start(Tag::Table(..)) => state.table = Some(TableState::default()),
            Event::End(TagEnd::Table) => {
                if let Some(table) = state.table.take() {
                    output.push_str(&self.render_table(&table));
                    output.push_str("\n\n");
                }
            }
            Event::Start(Tag::TableHead) => {
                if let Some(table) = state.table.as_mut() {
                    table.in_head = true;
                }
            }
            Event::End(TagEnd::TableHead) => {
                if let Some(table) = state.table.as_mut() {
                    table.finish_row();
                    table.in_head = false;
                }
            }
            Event::Start(Tag::TableRow) => {
                if let Some(table) = state.table.as_mut() {
                    table.current_row.clear();
                    table.current_cell.clear();
                }
            }
            Event::End(TagEnd::TableRow) => {
                if let Some(table) = state.table.as_mut() {
                    table.finish_row();
                }
            }
            Event::Start(Tag::TableCell) => {
                if let Some(table) = state.table.as_mut() {
                    table.current_cell.clear();
                }
            }
            Event::End(TagEnd::TableCell) => {
                if let Some(table) = state.table.as_mut() {
                    table.push_cell();
                }
            }
            Event::Start(Tag::Paragraph | Tag::MetadataBlock(..) | _)
            | Event::End(TagEnd::Image | TagEnd::MetadataBlock(..) | _) => {}
        }
    }

    #[allow(clippy::unused_self)]
    fn start_heading(&self, state: &mut RenderState, level: u8, output: &mut String) {
        state.heading_level = Some(level);
        if !output.is_empty() {
            output.push('\n');
        }
    }

    fn start_quote(&self, state: &mut RenderState, output: &mut String) {
        state.quote += 1;
        let _ = write!(output, "{}", "│ ".with(self.color_theme.quote));
    }

    fn start_item(state: &mut RenderState, output: &mut String) {
        let depth = state.list_stack.len().saturating_sub(1);
        output.push_str(&"  ".repeat(depth));

        let marker = match state.list_stack.last_mut() {
            Some(ListKind::Ordered { next_index }) => {
                let value = *next_index;
                *next_index += 1;
                format!("{value}. ")
            }
            _ => "• ".to_string(),
        };
        output.push_str(&marker);
    }

    fn start_code_block(&self, code_language: &str, output: &mut String) {
        let label = if code_language.is_empty() {
            "code".to_string()
        } else {
            code_language.to_string()
        };
        let _ = writeln!(
            output,
            "{}",
            format!("╭─ {label}")
                .bold()
                .with(self.color_theme.code_block_border)
        );
    }

    fn finish_code_block(&self, code_buffer: &str, code_language: &str, output: &mut String) {
        output.push_str(&self.highlight_code(code_buffer, code_language));
        let _ = write!(
            output,
            "{}",
            "╰─".bold().with(self.color_theme.code_block_border)
        );
        output.push_str("\n\n");
    }

    fn push_text(
        &self,
        text: &str,
        state: &mut RenderState,
        output: &mut String,
        code_buffer: &mut String,
        in_code_block: bool,
    ) {
        if in_code_block {
            code_buffer.push_str(text);
        } else {
            state.append_styled(output, text, &self.color_theme);
        }
    }

    fn render_table(&self, table: &TableState) -> String {
        let mut rows = Vec::new();
        if !table.headers.is_empty() {
            rows.push(table.headers.clone());
        }
        rows.extend(table.rows.iter().cloned());

        if rows.is_empty() {
            return String::new();
        }

        let column_count = rows.iter().map(Vec::len).max().unwrap_or(0);
        let widths = (0..column_count)
            .map(|column| {
                rows.iter()
                    .filter_map(|row| row.get(column))
                    .map(|cell| visible_width(cell))
                    .max()
                    .unwrap_or(0)
            })
            .collect::<Vec<_>>();

        let border = format!("{}", "│".with(self.color_theme.table_border));
        let separator = widths
            .iter()
            .map(|width| "─".repeat(*width + 2))
            .collect::<Vec<_>>()
            .join(&format!("{}", "┼".with(self.color_theme.table_border)));
        let separator = format!("{border}{separator}{border}");

        let mut output = String::new();
        if !table.headers.is_empty() {
            output.push_str(&self.render_table_row(&table.headers, &widths, true));
            output.push('\n');
            output.push_str(&separator);
            if !table.rows.is_empty() {
                output.push('\n');
            }
        }

        for (index, row) in table.rows.iter().enumerate() {
            output.push_str(&self.render_table_row(row, &widths, false));
            if index + 1 < table.rows.len() {
                output.push('\n');
            }
        }

        output
    }

    fn render_table_row(&self, row: &[String], widths: &[usize], is_header: bool) -> String {
        let border = format!("{}", "│".with(self.color_theme.table_border));
        let mut line = String::new();
        line.push_str(&border);

        for (index, width) in widths.iter().enumerate() {
            let cell = row.get(index).map_or("", String::as_str);
            line.push(' ');
            if is_header {
                let _ = write!(line, "{}", cell.bold().with(self.color_theme.heading));
            } else {
                line.push_str(cell);
            }
            let padding = width.saturating_sub(visible_width(cell));
            line.push_str(&" ".repeat(padding + 1));
            line.push_str(&border);
        }

        line
    }

    #[must_use]
    pub fn highlight_code(&self, code: &str, language: &str) -> String {
        let syntax = self
            .syntax_set
            .find_syntax_by_token(language)
            .unwrap_or_else(|| self.syntax_set.find_syntax_plain_text());
        let mut syntax_highlighter = HighlightLines::new(syntax, &self.syntax_theme);
        let mut colored_output = String::new();

        for line in LinesWithEndings::from(code) {
            match syntax_highlighter.highlight_line(line, &self.syntax_set) {
                Ok(ranges) => {
                    let escaped = as_24_bit_terminal_escaped(&ranges[..], false);
                    colored_output.push_str(&apply_code_block_background(&escaped));
                }
                Err(_) => colored_output.push_str(&apply_code_block_background(line)),
            }
        }

        colored_output
    }

    pub fn stream_markdown(&self, markdown: &str, out: &mut impl Write) -> io::Result<()> {
        let rendered_markdown = self.markdown_to_ansi(markdown);
        write!(out, "{rendered_markdown}")?;
        if !rendered_markdown.ends_with('\n') {
            writeln!(out)?;
        }
        out.flush()
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

fn apply_code_block_background(line: &str) -> String {
    let trimmed = line.trim_end_matches('\n');
    let trailing_newline = if trimmed.len() == line.len() {
        ""
    } else {
        "\n"
    };
    let with_background = trimmed.replace("\u{1b}[0m", "\u{1b}[0;48;5;236m");
    format!("\u{1b}[48;5;236m{with_background}\u{1b}[0m{trailing_newline}")
}

fn find_stream_safe_boundary(markdown: &str) -> Option<usize> {
    let mut in_fence = false;
    let mut last_boundary = None;

    // We yield at certain safe points to reduce perceived latency
    // Lines are safest, but for typewriter feel, word endings are better.
    for (offset, line) in markdown
        .split_inclusive(['\n', ' '])
        .scan(0usize, |cursor, part| {
            let start = *cursor;
            *cursor += part.len();
            Some((start, part))
        })
    {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            if !in_fence {
                last_boundary = Some(offset + line.len());
            }
            continue;
        }

        if in_fence {
            continue;
        }

        // Reverted to newline-only boundary for stability.
        // Real-time responsiveness will be handled by raw token streaming.
        if line.ends_with('\n') {
            last_boundary = Some(offset + line.len());
        }
    }

    last_boundary
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MarkerBuf {
    Empty,
    Star,
    Backtick,
    BacktickTwo,
    Tilde,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PredictiveLinkState {
    Inactive,
    ReadingText(String),
    ExpectingParen(String),
    ReadingUrl { text: String, url: String },
}

#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PredictiveMarkdownBuffer {
    bold: bool,
    italic: bool,
    strikethrough: bool,
    code_span: bool,

    code_block: bool,
    code_block_lang: String,
    code_block_buf: String,
    heading_level: u8,
    blockquote: bool,

    marker: MarkerBuf,
    link: PredictiveLinkState,

    at_line_start: bool,
    line_start_buf: String,
}

impl Default for PredictiveMarkdownBuffer {
    fn default() -> Self {
        Self {
            bold: false,
            italic: false,
            strikethrough: false,
            code_span: false,
            code_block: false,
            code_block_lang: String::new(),
            code_block_buf: String::new(),
            heading_level: 0,
            blockquote: false,
            marker: MarkerBuf::Empty,
            link: PredictiveLinkState::Inactive,
            at_line_start: true,
            line_start_buf: String::new(),
        }
    }
}

impl PredictiveMarkdownBuffer {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[cfg(test)]
    pub fn feed(&mut self, delta: &str, out: &mut String) {
        for c in delta.chars() {
            self.feed_char(c, out);
        }
    }

    pub fn flush(&mut self, out: &mut String) {
        self.flush_marker(out);
        self.flush_line_start(out);
        self.flush_link(out);
        if self.code_block {
            self.emit_code_block(out);
        }
        out.push_str("\x1b[0m");
        *self = Self::new();
    }

    pub fn feed_char(&mut self, c: char, out: &mut String) {
        if self.marker != MarkerBuf::Empty {
            self.resolve_marker(c, out);
            return;
        }

        if self.code_span {
            if c == '`' {
                self.code_span = false;
                self.emit_style_transition(out);
            } else {
                out.push(c);
            }
            return;
        }

        if self.code_block {
            self.code_block_feed(c, out);
            return;
        }

        if self.link != PredictiveLinkState::Inactive {
            self.link_feed(c, out);
            return;
        }

        if self.at_line_start {
            self.line_start_feed(c, out);
            return;
        }

        self.inline_char(c, out);
    }

    fn inline_char(&mut self, c: char, out: &mut String) {
        match c {
            '*' => self.marker = MarkerBuf::Star,
            '`' => self.marker = MarkerBuf::Backtick,
            '~' => self.marker = MarkerBuf::Tilde,
            '[' => {
                self.link = PredictiveLinkState::ReadingText(String::new());
            }
            '\n' => {
                self.heading_level = 0;
                self.blockquote = false;
                self.emit_style_transition(out);
                out.push('\n');
                self.at_line_start = true;
            }
            _ => {
                out.push(c);
            }
        }
    }

    fn resolve_marker(&mut self, c: char, out: &mut String) {
        match self.marker {
            MarkerBuf::Star => {
                self.marker = MarkerBuf::Empty;
                if c == '*' {
                    self.bold = !self.bold;
                    self.emit_style_transition(out);
                } else {
                    self.italic = !self.italic;
                    self.emit_style_transition(out);
                    self.inline_char(c, out);
                }
            }
            MarkerBuf::Backtick => {
                self.marker = MarkerBuf::Empty;
                if c == '`' {
                    self.marker = MarkerBuf::BacktickTwo;
                } else {
                    self.code_span = !self.code_span;
                    self.emit_style_transition(out);
                    if self.code_span {
                        out.push(c);
                    } else {
                        self.inline_char(c, out);
                    }
                }
            }
            MarkerBuf::BacktickTwo => {
                self.marker = MarkerBuf::Empty;
                if c == '`' || c == '\n' {
                    self.code_block = true;
                    self.code_block_lang.clear();
                    self.code_block_buf.clear();
                    if c != '\n' && c != '`' {
                        self.code_block_lang.push(c);
                    }
                } else {
                    out.push_str("``");
                    self.inline_char(c, out);
                }
            }
            MarkerBuf::Tilde => {
                self.marker = MarkerBuf::Empty;
                if c == '~' {
                    self.strikethrough = !self.strikethrough;
                    self.emit_style_transition(out);
                } else {
                    out.push('~');
                    self.inline_char(c, out);
                }
            }
            MarkerBuf::Empty => {}
        }
    }

    fn flush_marker(&mut self, out: &mut String) {
        match self.marker {
            MarkerBuf::Star => {
                if self.italic || self.bold {
                    if self.italic {
                        self.italic = false;
                    } else {
                        self.bold = false;
                    }
                    self.emit_style_transition(out);
                } else {
                    out.push('*');
                }
            }
            MarkerBuf::Backtick => {
                if self.code_span {
                    self.code_span = false;
                    self.emit_style_transition(out);
                } else {
                    out.push('`');
                }
            }
            MarkerBuf::BacktickTwo => out.push_str("``"),
            MarkerBuf::Tilde => out.push('~'),
            MarkerBuf::Empty => {}
        }
        self.marker = MarkerBuf::Empty;
    }

    fn code_block_feed(&mut self, c: char, out: &mut String) {
        if c == '\n' && self.code_block_buf.is_empty() && self.code_block_lang.is_empty() {
            return;
        }
        if c == '\n' && self.code_block_lang.is_empty() {
            return;
        }
        if c != '\n' && self.code_block_buf.is_empty() && !self.code_block_lang.contains('\n') {
            self.code_block_lang.push(c);
            return;
        }
        if !self.code_block_lang.contains('\n') {
            self.code_block_lang.push('\n');
        }

        self.code_block_buf.push(c);

        if c == '\n' {
            let is_closing = self.code_block_buf.lines().last().is_some_and(|line| {
                let trimmed = line.trim();
                trimmed == "```" || trimmed == "~~~"
            });
            if is_closing {
                let content_end = self
                    .code_block_buf
                    .rfind("\n```")
                    .or_else(|| self.code_block_buf.rfind("\n~~~"))
                    .unwrap_or(self.code_block_buf.len());
                let content = if content_end > 0 && content_end < self.code_block_buf.len() {
                    &self.code_block_buf[..content_end]
                } else {
                    ""
                };
                let lang = self.code_block_lang.trim();
                let label = if lang.is_empty() { "code" } else { lang };
                let _ = writeln!(out, "\x1b[1;90m╭─ {label}\x1b[0m");
                for line in content.lines() {
                    let _ = writeln!(out, "\x1b[48;5;236m{line}\x1b[0m");
                }
                let _ = write!(out, "\x1b[1;90m╰─\x1b[0m");

                self.code_block = false;
                self.code_block_buf.clear();
                self.code_block_lang.clear();
                self.at_line_start = true;
            }
        }
    }

    fn emit_code_block(&mut self, out: &mut String) {
        let lang = self.code_block_lang.trim();
        let label = if lang.is_empty() { "code" } else { lang };
        let _ = writeln!(out, "\x1b[1;90m╭─ {label}\x1b[0m");
        for line in self.code_block_buf.lines() {
            let _ = writeln!(out, "\x1b[48;5;236m{line}\x1b[0m");
        }
        let _ = write!(out, "\x1b[1;90m╰─\x1b[0m");
        self.code_block = false;
        self.code_block_buf.clear();
        self.code_block_lang.clear();
    }

    fn try_heading(buf: &str) -> Option<(u8, &str)> {
        const PREFIXES: &[(&str, u8)] = &[
            ("###### ", 6),
            ("##### ", 5),
            ("#### ", 4),
            ("### ", 3),
            ("## ", 2),
            ("# ", 1),
        ];
        for &(prefix, level) in PREFIXES {
            if let Some(rest) = buf.strip_prefix(prefix) {
                return Some((level, rest));
            }
        }
        None
    }

    fn line_start_feed(&mut self, c: char, out: &mut String) {
        self.line_start_buf.push(c);
        let buf = &self.line_start_buf;

        if buf.chars().all(|ch| ch == '#') && buf.len() <= 6 {
            return;
        }

        if let Some((level, rest)) = Self::try_heading(buf) {
            let rest = rest.to_string();
            self.heading_level = level;
            self.emit_style_transition(out);
            out.push_str(&rest);
            self.line_start_buf.clear();
            self.at_line_start = false;
            return;
        }

        if buf == "- " || buf == "* " || buf == "+ " {
            out.push_str("• ");
            self.line_start_buf.clear();
            self.at_line_start = false;
            return;
        }
        if buf == "-" || buf == "+" || buf == "*" {
            return;
        }
        if buf == "> " {
            self.blockquote = true;
            self.emit_style_transition(out);
            out.push_str("│ ");
            self.line_start_buf.clear();
            self.at_line_start = false;
            return;
        }
        if buf == ">" {
            return;
        }

        if buf.len() >= 2 && buf.ends_with(". ") {
            let prefix = &buf[..buf.len() - 2];
            if prefix.chars().all(|ch| ch.is_ascii_digit()) {
                out.push_str(buf);
                self.line_start_buf.clear();
                self.at_line_start = false;
                return;
            }
        }
        if buf.chars().all(|ch| ch.is_ascii_digit()) {
            return;
        }
        if buf.len() >= 2
            && buf.ends_with('.')
            && buf[..buf.len() - 1].chars().all(|ch| ch.is_ascii_digit())
        {
            return;
        }

        if buf.starts_with("```") || buf.starts_with("~~~") {
            if c == '\n' {
                self.code_block = true;
                let lang_part = &buf[3..];
                self.code_block_lang = lang_part.trim_end_matches('\n').to_string();
                self.code_block_buf.clear();
                self.line_start_buf.clear();
                self.at_line_start = false;
            }
            return;
        }
        if buf == "`" || buf == "``" || buf == "~" || buf == "~~" {
            return;
        }

        self.flush_line_start(out);
    }

    fn flush_line_start(&mut self, out: &mut String) {
        if self.line_start_buf.is_empty() {
            return;
        }
        let buf = std::mem::take(&mut self.line_start_buf);
        self.at_line_start = false;
        for c in buf.chars() {
            self.feed_char(c, out);
        }
    }

    fn link_feed(&mut self, c: char, out: &mut String) {
        let next = match std::mem::replace(&mut self.link, PredictiveLinkState::Inactive) {
            PredictiveLinkState::ReadingText(mut text) => {
                if c == ']' {
                    PredictiveLinkState::ExpectingParen(text)
                } else {
                    text.push(c);
                    PredictiveLinkState::ReadingText(text)
                }
            }
            PredictiveLinkState::ExpectingParen(text) => {
                if c == '(' {
                    PredictiveLinkState::ReadingUrl {
                        text,
                        url: String::new(),
                    }
                } else {
                    out.push('[');
                    out.push_str(&text);
                    out.push(']');
                    self.link = PredictiveLinkState::Inactive;
                    self.inline_char(c, out);
                    return;
                }
            }
            PredictiveLinkState::ReadingUrl { text, mut url } => {
                if c == ')' {
                    let _ = write!(out, "\x1b[4;34m[{text}]({url})\x1b[0m");
                    self.emit_style_transition(out);
                    self.link = PredictiveLinkState::Inactive;
                    return;
                }
                url.push(c);
                PredictiveLinkState::ReadingUrl { text, url }
            }
            PredictiveLinkState::Inactive => {
                self.inline_char(c, out);
                return;
            }
        };
        self.link = next;
    }

    fn flush_link(&mut self, out: &mut String) {
        match std::mem::replace(&mut self.link, PredictiveLinkState::Inactive) {
            PredictiveLinkState::ReadingText(text) => {
                out.push('[');
                out.push_str(&text);
            }
            PredictiveLinkState::ExpectingParen(text) => {
                out.push('[');
                out.push_str(&text);
                out.push(']');
            }
            PredictiveLinkState::ReadingUrl { text, url } => {
                out.push('[');
                out.push_str(&text);
                out.push_str("](");
                out.push_str(&url);
            }
            PredictiveLinkState::Inactive => {}
        }
    }

    fn emit_style_transition(&self, out: &mut String) {
        out.push_str("\x1b[0m");
        let mut sgr = Vec::new();
        if self.bold || matches!(self.heading_level, 1 | 2) {
            sgr.push("1");
        }
        if self.italic {
            sgr.push("3");
        }
        if self.strikethrough {
            sgr.push("9");
        }
        if self.code_span {
            sgr.push("32");
        } else {
            match self.heading_level {
                1 => sgr.push("36"),
                2 => sgr.push("37"),
                3 => sgr.push("34"),
                h if h > 0 => sgr.push("90"),
                _ if self.bold => sgr.push("33"),
                _ if self.italic => sgr.push("35"),
                _ if self.blockquote => sgr.push("90"),
                _ => {}
            }
        }
        if !sgr.is_empty() {
            let _ = write!(out, "\x1b[{}m", sgr.join(";"));
        }
    }
}

fn visible_width(input: &str) -> usize {
    text_display_width(&strip_ansi(input))
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
        strip_ansi, MarkdownStreamState, PredictiveMarkdownBuffer, Spinner, TerminalRenderer,
    };

    #[test]
    fn renders_markdown_with_styling_and_lists() {
        let terminal_renderer = TerminalRenderer::new();
        let markdown_output = terminal_renderer
            .render_markdown("# Heading\n\nThis is **bold** and *italic*.\n\n- item\n\n`code`");

        assert!(markdown_output.contains("Heading"));
        assert!(markdown_output.contains("• item"));
        assert!(markdown_output.contains("code"));
        assert!(markdown_output.contains('\u{1b}'));
    }

    #[test]
    fn renders_links_as_colored_markdown_labels() {
        let terminal_renderer = TerminalRenderer::new();
        let markdown_output =
            terminal_renderer.render_markdown("See [Docs](https://example.com/docs) now.");
        let plain_text = strip_ansi(&markdown_output);

        assert!(plain_text.contains("[Docs](https://example.com/docs)"));
        assert!(markdown_output.contains('\u{1b}'));
    }

    #[test]
    fn highlights_fenced_code_blocks() {
        let terminal_renderer = TerminalRenderer::new();
        let markdown_output =
            terminal_renderer.markdown_to_ansi("```rust\nfn hi() { println!(\"hi\"); }\n```");
        let plain_text = strip_ansi(&markdown_output);

        assert!(plain_text.contains("╭─ rust"));
        assert!(plain_text.contains("fn hi"));
        assert!(markdown_output.contains('\u{1b}'));
        assert!(markdown_output.contains("[48;5;236m"));
    }

    #[test]
    fn renders_ordered_and_nested_lists() {
        let terminal_renderer = TerminalRenderer::new();
        let markdown_output =
            terminal_renderer.render_markdown("1. first\n2. second\n   - nested\n   - child");
        let plain_text = strip_ansi(&markdown_output);

        assert!(plain_text.contains("1. first"));
        assert!(plain_text.contains("2. second"));
        assert!(plain_text.contains("  • nested"));
        assert!(plain_text.contains("  • child"));
    }

    #[test]
    fn renders_tables_with_alignment() {
        let terminal_renderer = TerminalRenderer::new();
        let markdown_output = terminal_renderer
            .render_markdown("| Name | Value |\n| ---- | ----- |\n| alpha | 1 |\n| beta | 22 |");
        let plain_text = strip_ansi(&markdown_output);
        let lines = plain_text.lines().collect::<Vec<_>>();

        assert_eq!(lines[0], "│ Name  │ Value │");
        assert_eq!(lines[1], "│───────┼───────│");
        assert_eq!(lines[2], "│ alpha │ 1     │");
        assert_eq!(lines[3], "│ beta  │ 22    │");
        assert!(markdown_output.contains('\u{1b}'));
    }

    #[test]
    fn renders_tables_with_wide_chars_aligned() {
        let terminal_renderer = TerminalRenderer::new();
        let markdown_output = terminal_renderer
            .render_markdown("| Name | Value |\n| ---- | ----- |\n| 中文 | 1 |\n| a | 22 |");
        let plain_text = strip_ansi(&markdown_output);
        let lines = plain_text.lines().collect::<Vec<_>>();

        assert_eq!(lines[0], "│ Name │ Value │");
        assert_eq!(lines[1], "│──────┼───────│");
        assert_eq!(lines[2], "│ 中文 │ 1     │");
        assert_eq!(lines[3], "│ a    │ 22    │");
    }

    #[test]
    fn streaming_state_waits_for_complete_blocks() {
        let renderer = TerminalRenderer::new();
        let mut state = MarkdownStreamState::default();

        assert_eq!(state.push(&renderer, "# Heading"), None);
        let flushed = state
            .push(&renderer, "\n\nParagraph\n\n")
            .expect("completed block");
        let plain_text = strip_ansi(&flushed);
        assert!(plain_text.contains("Heading"));
        assert!(plain_text.contains("Paragraph"));

        assert_eq!(state.push(&renderer, "```rust\nfn main() {}\n"), None);
        let code = state
            .push(&renderer, "```\n")
            .expect("closed code fence flushes");
        assert!(strip_ansi(&code).contains("fn main()"));
    }

    #[test]
    fn spinner_advances_frames() {
        let terminal_renderer = TerminalRenderer::new();
        let mut spinner = Spinner::new();
        let mut out = Vec::new();
        spinner
            .tick("Working", terminal_renderer.color_theme(), &mut out)
            .expect("tick succeeds");
        spinner
            .tick("Working", terminal_renderer.color_theme(), &mut out)
            .expect("tick succeeds");

        let output = String::from_utf8_lossy(&out);
        assert!(output.contains("Working"));
    }

    // ── PredictiveMarkdownBuffer tests ──────────────────────────────

    fn feed(input: &str) -> String {
        let mut buf = PredictiveMarkdownBuffer::new();
        let mut out = String::new();
        buf.feed(input, &mut out);
        buf.flush(&mut out);
        out
    }

    fn feed_plain(input: &str) -> String {
        strip_ansi(&feed(input))
    }

    // -- plain text passthrough --

    #[test]
    fn predictive_plain_text_passes_through() {
        assert_eq!(feed_plain("hello world"), "hello world");
    }

    #[test]
    fn predictive_newline_passes_through() {
        assert_eq!(feed_plain("a\nb"), "a\nb");
    }

    // -- bold toggle --

    #[test]
    fn predictive_bold_produces_ansi() {
        let out = feed("**bold**");
        assert!(out.contains('\x1b'), "should contain ANSI codes");
        assert_eq!(strip_ansi(&out), "bold");
    }

    #[test]
    fn predictive_bold_toggle_on_off() {
        let out = feed("before **bold** after");
        let plain = strip_ansi(&out);
        assert_eq!(plain, "before bold after");
        assert!(out.contains('\x1b'));
    }

    #[test]
    fn predictive_bold_streamed_char_by_char() {
        let mut buf = PredictiveMarkdownBuffer::new();
        let mut out = String::new();
        for c in "**hi**".chars() {
            buf.feed_char(c, &mut out);
        }
        buf.flush(&mut out);
        assert_eq!(strip_ansi(&out), "hi");
        assert!(out.contains('\x1b'));
    }

    // -- italic toggle --

    #[test]
    fn predictive_italic_produces_ansi() {
        let out = feed("*italic*");
        assert_eq!(strip_ansi(&out), "italic");
        assert!(out.contains('\x1b'));
    }

    #[test]
    fn predictive_italic_toggle_on_off() {
        let out = feed("before *italic* after");
        assert_eq!(strip_ansi(&out), "before italic after");
    }

    // -- bold+italic mixed --

    #[test]
    fn predictive_bold_and_italic_independent() {
        let out = feed("**bold *both* bold**");
        let plain = strip_ansi(&out);
        assert_eq!(plain, "bold both bold");
    }

    // -- code span toggle --

    #[test]
    fn predictive_code_span() {
        let out = feed("use `code` here");
        assert_eq!(strip_ansi(&out), "use code here");
        assert!(out.contains('\x1b'));
    }

    #[test]
    fn predictive_code_span_suppresses_markers() {
        let out = feed("`**not bold**`");
        let plain = strip_ansi(&out);
        assert_eq!(plain, "**not bold**");
    }

    // -- strikethrough toggle --

    #[test]
    fn predictive_strikethrough() {
        let out = feed("~~struck~~");
        assert_eq!(strip_ansi(&out), "struck");
        assert!(out.contains('\x1b'));
    }

    // -- marker disambiguation --

    #[test]
    fn predictive_lone_star_is_italic() {
        let out = feed("*a* b");
        assert_eq!(strip_ansi(&out), "a b");
    }

    #[test]
    fn predictive_double_star_is_bold() {
        let out = feed("**a** b");
        assert_eq!(strip_ansi(&out), "a b");
    }

    #[test]
    fn predictive_lone_tilde_is_literal() {
        assert_eq!(feed_plain("~x"), "~x");
    }

    #[test]
    fn predictive_lone_backtick_is_code_span() {
        let out = feed("`x`");
        assert_eq!(strip_ansi(&out), "x");
    }

    // -- headings --

    #[test]
    fn predictive_heading_level_1() {
        let out = feed("# Title\n");
        let plain = strip_ansi(&out);
        assert_eq!(plain, "Title\n");
        assert!(out.contains('\x1b'));
    }

    #[test]
    fn predictive_heading_level_2() {
        let out = feed("## Sub\n");
        assert_eq!(strip_ansi(&out), "Sub\n");
    }

    #[test]
    fn predictive_heading_resets_after_newline() {
        let out = feed("# H\nnormal");
        let plain = strip_ansi(&out);
        assert_eq!(plain, "H\nnormal");
    }

    // -- lists --

    #[test]
    fn predictive_unordered_list_dash() {
        let out = feed("- item\n");
        let plain = strip_ansi(&out);
        assert_eq!(plain, "• item\n");
    }

    #[test]
    fn predictive_unordered_list_star() {
        let out = feed("* item\n");
        let plain = strip_ansi(&out);
        assert_eq!(plain, "• item\n");
    }

    #[test]
    fn predictive_ordered_list() {
        let out = feed("1. first\n");
        let plain = strip_ansi(&out);
        assert_eq!(plain, "1. first\n");
    }

    // -- blockquote --

    #[test]
    fn predictive_blockquote() {
        let out = feed("> quoted\n");
        let plain = strip_ansi(&out);
        assert_eq!(plain, "│ quoted\n");
    }

    // -- code block --

    #[test]
    fn predictive_code_block_buffered() {
        let mut buf = PredictiveMarkdownBuffer::new();
        let mut out = String::new();
        buf.feed("```\ncode line\n```\n", &mut out);
        buf.flush(&mut out);
        let plain = strip_ansi(&out);
        assert!(plain.contains("code line"));
    }

    #[test]
    fn predictive_code_block_suppresses_inline_markers() {
        let out = feed("```\n**not bold**\n```\n");
        let plain = strip_ansi(&out);
        assert!(plain.contains("**not bold**"));
    }

    #[test]
    fn predictive_code_block_with_language() {
        let out = feed("```rust\nfn main() {}\n```\n");
        let plain = strip_ansi(&out);
        assert!(plain.contains("fn main()"));
    }

    // -- links --

    #[test]
    fn predictive_link_renders() {
        let out = feed("[click](https://example.com)");
        let plain = strip_ansi(&out);
        assert!(plain.contains("click"));
        assert!(plain.contains("https://example.com"));
    }

    #[test]
    fn predictive_link_aborted_no_paren() {
        let out = feed("[text] rest");
        let plain = strip_ansi(&out);
        assert_eq!(plain, "[text] rest");
    }

    // -- flush --

    #[test]
    fn predictive_flush_emits_pending_marker() {
        let mut buf = PredictiveMarkdownBuffer::new();
        let mut out = String::new();
        buf.feed("trailing*", &mut out);
        buf.flush(&mut out);
        assert_eq!(strip_ansi(&out), "trailing*");
    }

    #[test]
    fn predictive_flush_resets_state() {
        let mut buf = PredictiveMarkdownBuffer::new();
        let mut out = String::new();
        buf.feed("**bold not closed", &mut out);
        buf.flush(&mut out);
        let mut out2 = String::new();
        buf.feed("new text", &mut out2);
        buf.flush(&mut out2);
        let plain2 = strip_ansi(&out2);
        assert_eq!(plain2, "new text");
    }
}
