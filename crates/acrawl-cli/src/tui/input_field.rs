use std::time::Instant;

use crate::display_width::{char_count_for_display_col, char_display_width, text_display_width};

/// A reusable text-editing field with cursor tracking, selection, undo/redo,
/// paste-burst detection, and visual-line navigation.
///
/// Intended to replace all hand-written buffer+cursor+helper patterns across
/// the TUI (main input, filter fields, rename dialogs, etc.).
#[derive(Clone, Debug)]
pub struct InputField {
    text: String,
    cursor: usize,
    byte_cursor: usize,
    selection: Option<(usize, usize)>,
    preferred_col: Option<usize>,
    undo_stack: Vec<Snapshot>,
    redo_stack: Vec<Snapshot>,
    paste_threshold_ms: Option<u64>,
    paste_buffer: Option<(Instant, Vec<char>)>,
    last_key_time: Option<Instant>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Snapshot {
    pub text: String,
    pub cursor: usize,
    pub preferred_col: Option<usize>,
    pub selection: Option<(usize, usize)>,
}

const PASTE_THRESHOLD_MS: u64 = 30;

impl InputField {
    pub fn new() -> Self {
        Self {
            text: String::new(),
            cursor: 0,
            byte_cursor: 0,
            selection: None,
            preferred_col: None,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            paste_threshold_ms: None,
            paste_buffer: None,
            last_key_time: None,
        }
    }

    pub fn with_undo(mut self) -> Self {
        self.undo_stack = Vec::new();
        self
    }

    pub fn with_paste_detection(mut self) -> Self {
        self.paste_threshold_ms = Some(PASTE_THRESHOLD_MS);
        self
    }

    pub fn with_text(mut self, text: impl Into<String>) -> Self {
        let t = text.into();
        self.text = t;
        self.cursor = self.char_len();
        self.resync_byte_cursor();
        self
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn preferred_col(&self) -> Option<usize> {
        self.preferred_col
    }

    pub fn set_preferred_col(&mut self, col: Option<usize>) {
        self.preferred_col = col;
    }

    pub fn selection(&self) -> Option<(usize, usize)> {
        self.selection
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub fn char_len(&self) -> usize {
        self.text.chars().count()
    }

    pub fn char_to_byte(&self, char_idx: usize) -> usize {
        self.text
            .char_indices()
            .nth(char_idx)
            .map_or(self.text.len(), |(idx, _)| idx)
    }

    pub fn resync_byte_cursor(&mut self) {
        self.byte_cursor = self
            .text
            .char_indices()
            .nth(self.cursor)
            .map_or(self.text.len(), |(idx, _)| idx);
    }

    pub fn clamp_cursor(&mut self) {
        let old = self.cursor;
        self.cursor = self.cursor.min(self.char_len());
        if self.cursor != old {
            self.resync_byte_cursor();
        }
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
        self.byte_cursor = 0;
        self.selection = None;
        self.preferred_col = None;
    }

    pub fn has_selection(&self) -> bool {
        self.selection.is_some()
    }

    pub fn selected_text(&self) -> Option<&str> {
        let (a, b) = self.selection?;
        let sel_start = self.char_to_byte(a);
        let sel_end = self.char_to_byte(b);
        self.text.get(sel_start..sel_end)
    }

    pub fn delete_selection(&mut self) -> bool {
        if let Some((a, b)) = self.selection.take() {
            self.cursor = a;
            self.resync_byte_cursor();
            let end_byte = self.char_to_byte(b);
            self.text.replace_range(self.byte_cursor..end_byte, "");
            self.preferred_col = None;
            true
        } else {
            false
        }
    }

    pub fn clear_selection(&mut self) {
        self.selection = None;
    }

    pub fn set_selection(&mut self, start: usize, end: usize) {
        self.selection = if start < end {
            Some((start, end))
        } else {
            None
        };
    }

    pub fn cut_selected(&mut self) -> Option<String> {
        let text = self.selected_text()?.to_string();
        self.record_undo();
        self.delete_selection();
        Some(text)
    }

    pub fn select_all(&mut self) {
        let char_len = self.char_len();
        if char_len == 0 {
            return;
        }
        self.selection = Some((0, char_len));
        self.cursor = char_len;
        self.resync_byte_cursor();
        self.preferred_col = None;
    }

    pub fn insert_char(&mut self, ch: char) {
        self.record_undo();
        self.delete_selection();
        self.clamp_cursor();
        self.text.insert(self.byte_cursor, ch);
        self.cursor = self.cursor.saturating_add(1);
        self.byte_cursor = self.byte_cursor.saturating_add(ch.len_utf8());
        self.preferred_col = None;
    }

    pub fn insert_str(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.record_undo();
        self.delete_selection();
        self.clamp_cursor();
        self.text.insert_str(self.byte_cursor, text);
        let char_count = text.chars().count();
        self.cursor = self.cursor.saturating_add(char_count);
        self.byte_cursor = self.byte_cursor.saturating_add(text.len());
        self.preferred_col = None;
    }

    pub fn backspace(&mut self) {
        let had_selection = self.selection.is_some();
        self.clamp_cursor();
        if !had_selection && self.cursor == 0 {
            return;
        }
        self.record_undo();
        if self.delete_selection() {
            return;
        }
        let prev_byte = if self.byte_cursor > 0 {
            let bytes = self.text.as_bytes();
            let mut pos = self.byte_cursor - 1;
            while pos > 0 && (bytes[pos] & 0xC0) == 0x80 {
                pos -= 1;
            }
            pos
        } else {
            0
        };
        let start = prev_byte;
        let end = self.byte_cursor;
        self.text.replace_range(start..end, "");
        self.cursor -= 1;
        self.byte_cursor = start;
        self.preferred_col = None;
    }

    pub fn delete(&mut self) {
        let had_selection = self.selection.is_some();
        self.clamp_cursor();
        if !had_selection && self.cursor >= self.char_len() {
            return;
        }
        self.record_undo();
        if self.delete_selection() {
            return;
        }
        let bytes = self.text.as_bytes();
        let mut end = self.byte_cursor + 1;
        while end < bytes.len() && (bytes[end] & 0xC0) == 0x80 {
            end += 1;
        }
        self.text.replace_range(self.byte_cursor..end, "");
        self.preferred_col = None;
    }

    pub fn move_cursor_left(&mut self) {
        self.selection = None;
        if self.cursor == 0 {
            return;
        }
        let bytes = self.text.as_bytes();
        let mut pos = self.byte_cursor.saturating_sub(1);
        while pos > 0 && (bytes[pos] & 0xC0) == 0x80 {
            pos -= 1;
        }
        self.byte_cursor = pos;
        self.cursor -= 1;
        self.preferred_col = None;
    }

    pub fn move_cursor_right(&mut self) {
        self.selection = None;
        if self.cursor >= self.char_len() {
            return;
        }
        let bytes = self.text.as_bytes();
        let mut end = self.byte_cursor + 1;
        while end < bytes.len() && (bytes[end] & 0xC0) == 0x80 {
            end += 1;
        }
        self.byte_cursor = end;
        self.cursor += 1;
        self.preferred_col = None;
    }

    pub fn move_cursor_home(&mut self) {
        let (line, _) = self.cursor_line_col();
        self.set_cursor_by_char_from_line_col(line, 0);
        self.preferred_col = Some(0);
    }

    pub fn move_cursor_end(&mut self) {
        let (line, _) = self.cursor_line_col();
        let target = self.lines().get(line).map_or(0, |l| text_display_width(l));
        self.set_cursor_by_char_from_line_col(line, target);
        self.preferred_col = Some(target);
    }

    pub fn set_cursor_by_char(&mut self, char_idx: usize) {
        self.selection = None;
        self.cursor = char_idx.min(self.char_len());
        self.resync_byte_cursor();
        self.preferred_col = None;
    }

    pub fn cursor_line_col(&self) -> (usize, usize) {
        let mut line = 0usize;
        let mut col = 0usize;
        for (idx, ch) in self.text.chars().enumerate() {
            if idx == self.cursor {
                break;
            }
            if ch == '\n' {
                line += 1;
                col = 0;
            } else {
                col += char_display_width(ch);
            }
        }
        (line, col)
    }

    pub fn lines(&self) -> Vec<&str> {
        self.text.split('\n').collect()
    }

    pub fn set_cursor_by_char_from_line_col(&mut self, target_line: usize, target_col: usize) {
        let lines = self.lines();
        let line = target_line.min(lines.len().saturating_sub(1));
        let col = char_count_for_display_col(lines[line], target_col);
        let mut cursor = 0usize;
        for input_line in lines.iter().take(line) {
            cursor += input_line.chars().count() + 1;
        }
        cursor += col;
        self.cursor = cursor.min(self.char_len());
        self.resync_byte_cursor();
        self.preferred_col = None;
    }

    pub fn visual_line_info(&self, safe_width: usize, prompt_offset: usize) -> Vec<(usize, usize)> {
        let mut lines = Vec::new();
        let mut char_idx = 0usize;
        let parts: Vec<&str> = self.text.split('\n').collect();
        for (logical_idx, logical_line) in parts.iter().enumerate() {
            let logical_chars: Vec<char> = logical_line.chars().collect();
            let first_offset = if logical_idx == 0 { prompt_offset } else { 0 };
            let first_cap = safe_width.saturating_sub(first_offset);
            let cap = safe_width;
            let mut offset = 0usize;
            loop {
                let remaining = logical_chars.len().saturating_sub(offset);
                if remaining == 0 {
                    if offset == 0 {
                        lines.push((char_idx, 0));
                    }
                    break;
                }
                let w = if offset == 0 { first_cap } else { cap };
                let mut col = 0usize;
                let mut end = offset;
                while end < logical_chars.len() {
                    let cw = char_display_width(logical_chars[end]);
                    if col + cw > w {
                        break;
                    }
                    col += cw;
                    end += 1;
                }
                lines.push((char_idx + offset, col));
                if end == offset {
                    end = offset + 1;
                }
                offset = end;
            }
            char_idx += logical_chars.len();
            if logical_idx < parts.len() - 1 {
                char_idx += 1;
            }
        }
        lines
    }

    #[allow(dead_code)]
    pub fn char_index_at_visual_pos(
        &self,
        row: usize,
        col: usize,
        prompt_offset: usize,
        visual_lines: &[(usize, usize)],
    ) -> Option<usize> {
        let &(start, width) = visual_lines.get(row)?;
        let prompt = if row == 0 { prompt_offset } else { 0 };
        if col < prompt {
            return Some(start);
        }
        let target_col = col.saturating_sub(prompt).min(width);
        let raw_end = visual_lines.get(row + 1).map_or(self.char_len(), |v| v.0);
        let line_end =
            if raw_end > start && self.text.chars().nth(raw_end.saturating_sub(1)) == Some('\n') {
                raw_end.saturating_sub(1)
            } else {
                raw_end
            };
        let mut char_idx = start;
        let mut cur_col = 0usize;
        for ch in self
            .text
            .chars()
            .skip(start)
            .take(line_end.saturating_sub(start))
        {
            let cw = char_display_width(ch);
            if cw == 0 {
                continue;
            }
            if cur_col + cw > target_col {
                break;
            }
            cur_col += cw;
            char_idx += 1;
        }
        Some(char_idx)
    }

    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            text: self.text.clone(),
            cursor: self.cursor,
            preferred_col: self.preferred_col,
            selection: self.selection,
        }
    }

    pub fn apply_snapshot(&mut self, snapshot: Snapshot) {
        self.text = snapshot.text;
        self.cursor = snapshot.cursor.min(self.char_len());
        self.preferred_col = snapshot.preferred_col;
        self.selection = snapshot.selection;
        self.resync_byte_cursor();
    }

    pub fn record_undo(&mut self) {
        let snapshot = self.snapshot();
        if self.undo_stack.last() != Some(&snapshot) {
            self.undo_stack.push(snapshot);
        }
        self.redo_stack.clear();
    }

    pub fn undo(&mut self) -> bool {
        let Some(snapshot) = self.undo_stack.pop() else {
            return false;
        };
        self.redo_stack.push(self.snapshot());
        self.apply_snapshot(snapshot);
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(snapshot) = self.redo_stack.pop() else {
            return false;
        };
        self.undo_stack.push(self.snapshot());
        self.apply_snapshot(snapshot);
        true
    }

    pub fn clear_undo_history(&mut self) {
        self.undo_stack.clear();
        self.redo_stack.clear();
    }

    /// Call this before processing a key event so the paste burst detector
    /// can measure inter-key timing.
    pub fn note_key_time(&mut self, now: Instant) {
        self.last_key_time = Some(now);
    }

    #[allow(dead_code)]
    pub fn feed_paste_char(&mut self, ch: char) -> Option<String> {
        let threshold = self.paste_threshold_ms?;
        let now = Instant::now();

        let is_burst = self.last_key_time.is_some_and(|t| {
            u64::try_from(now.duration_since(t).as_millis()).unwrap_or(u64::MAX) <= threshold
        });

        let is_immediate_paste = self.last_key_time.is_none();

        if is_burst || is_immediate_paste {
            self.paste_buffer
                .get_or_insert_with(|| (now, Vec::new()))
                .1
                .push(ch);
            self.last_key_time = Some(now);
            None
        } else {
            let mut result = String::new();
            if let Some((_, chars)) = self.paste_buffer.take() {
                for c in chars {
                    result.push(c);
                }
            }
            result.push(ch);
            Some(result)
        }
    }

    /// Flush any pending paste-burst accumulator directly into the text buffer.
    /// Returns `true` if text was inserted.
    pub fn flush_paste_burst(&mut self) -> bool {
        let Some((_, chars)) = self.paste_buffer.take() else {
            return false;
        };
        let n = chars.len();
        if n == 0 {
            return false;
        }
        self.record_undo();
        self.delete_selection();
        self.clamp_cursor();
        let prefix = &self.text[..self.byte_cursor];
        let suffix = &self.text[self.byte_cursor..];
        let mut pasted_len = 0usize;
        let cap = prefix.len() + suffix.len() + n;
        let mut text = String::with_capacity(cap);
        text.push_str(prefix);
        for c in &chars {
            pasted_len += c.len_utf8();
            text.push(*c);
        }
        text.push_str(suffix);
        self.byte_cursor = self.byte_cursor.saturating_add(pasted_len);
        self.cursor = self.cursor.saturating_add(n);
        self.text = text;
        self.preferred_col = None;
        true
    }

    pub fn take_text(&mut self) -> String {
        std::mem::take(&mut self.text)
    }

    /// Returns true if a paste burst is currently being accumulated.
    pub fn paste_burst_active(&self) -> bool {
        self.paste_buffer.is_some()
    }

    /// Returns true if a paste burst is active AND the last key time is set.
    pub fn paste_burst_and_key_active(&self) -> bool {
        self.paste_buffer.is_some() && self.last_key_time.is_some()
    }

    /// Returns `true` when the most-recent key event is within `threshold_ms`
    /// of `now` (i.e. the next character likely belongs to the same paste burst).
    pub fn last_key_within(&self, now: Instant, threshold_ms: u64) -> bool {
        self.last_key_time.is_some_and(|t| {
            u64::try_from(now.duration_since(t).as_millis()).unwrap_or(u64::MAX) <= threshold_ms
        })
    }

    pub fn last_key_older_than(&self, now: Instant, threshold_ms: u64) -> bool {
        self.last_key_time.is_some_and(|t| {
            u64::try_from(now.duration_since(t).as_millis()).unwrap_or(u64::MAX) > threshold_ms
        })
    }

    /// Push a character into the paste burst accumulator, recording `now` as
    /// the key time.
    pub fn push_paste_char(&mut self, c: char, now: Instant) {
        self.paste_buffer
            .get_or_insert_with(|| (now, Vec::new()))
            .1
            .push(c);
        self.last_key_time = Some(now);
    }

    /// Clear both the paste buffer and the last-key timestamp.
    pub fn reset_paste_state(&mut self) {
        self.paste_buffer = None;
        self.last_key_time = None;
    }
}

impl Default for InputField {
    fn default() -> Self {
        Self::new()
    }
}
