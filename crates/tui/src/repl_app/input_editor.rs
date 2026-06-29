use super::*;

impl ReplTuiState {
    pub(super) fn tick_input_caret(&mut self) {
        let now = Instant::now();
        let advance_spinner = now >= self.spinner_deadline;
        if now >= self.cursor_blink_deadline {
            self.cursor_on = true;
            self.cursor_blink_deadline = now + Duration::from_millis(530);
        }
        if advance_spinner {
            self.spinner_tick = self.spinner_tick.wrapping_add(1);
            self.spinner_deadline = now + Duration::from_millis(120);
        }
        if let Some(modal) = self
            .active_modal
            .as_mut()
            .and_then(ActiveModal::as_auth_mut)
        {
            if let AuthModalStep::OAuthWaiting { tick, .. } = &mut modal.step {
                if advance_spinner {
                    *tick = tick.wrapping_add(1);
                }
            }
        }
    }

    /// Returns the spinner frame matching the current tick.
    pub(crate) fn spinner_char(&self) -> char {
        const FRAMES: [char; 8] = [
            '\u{280B}', '\u{2819}', '\u{2839}', '\u{2838}', '\u{283C}', '\u{2834}', '\u{2826}',
            '\u{2827}',
        ];
        if self.cancelling {
            return '\u{25FC}';
        }
        FRAMES[usize::from(self.spinner_tick) % FRAMES.len()]
    }

    /// Context-aware placeholder shown when the input box is empty.
    pub(super) fn input_placeholder(&self) -> &'static str {
        if self.busy {
            "AgenticCrawler is working    (you can queue your next prompt)"
        } else if self.ui_state == AppUiState::WelcomeMode {
            "What is our goal today?"
        } else {
            "Any follow-up instructions?"
        }
    }

    /// Advance the typewriter: reveal `chars_per_tick` chars from the queue.
    ///
    /// Chars accumulate in `typewriter.live`; whenever the buffer reaches a
    /// stream-safe markdown boundary (paragraph end or closed code fence) we
    /// render that chunk through `tui-markdown` and push the resulting lines
    /// to the transcript. This preserves multi-line constructs like fenced
    /// code blocks (which need the whole block to syntax-highlight) at the
    /// cost of a slight delay before they appear during streaming.
    pub(super) fn tick_typewriter(&mut self, chars_per_tick: usize) {
        for _ in 0..chars_per_tick {
            match self.typewriter.chars.pop_front() {
                None => break,
                Some(c) => self.typewriter.live.push(c),
            }
        }
    }

    pub(super) fn flush_typewriter(&mut self) {
        if !self.typewriter.chars.is_empty() {
            let count = self.typewriter.chars.len();
            self.tick_typewriter(count);
        }
    }

    pub(super) fn wake_input_caret(&mut self) {
        self.cursor_on = true;
        self.cursor_blink_deadline = Instant::now() + Duration::from_millis(530);
    }

    pub(super) fn current_input_snapshot(&self) -> InputUndoSnapshot {
        InputUndoSnapshot {
            text: self.input.text.clone(),
            cursor: self.input.cursor,
            preferred_col: self.input.preferred_col,
            selection: self.input_selection,
            pastes: self.input.pastes.clone(),
            next_paste_id: self.input.next_paste_id,
        }
    }

    pub(super) fn apply_input_snapshot(&mut self, snapshot: InputUndoSnapshot) {
        self.input_scroll_manual = false;
        self.input.text = snapshot.text;
        self.input.cursor = snapshot.cursor.min(self.input_char_len());
        self.input.preferred_col = snapshot.preferred_col;
        self.input_selection = snapshot.selection;
        self.input_click_anchor = None;
        self.input.pastes = snapshot.pastes;
        self.input.next_paste_id = snapshot.next_paste_id;
        self.resync_byte_cursor();
        self.input_scroll_offset = usize::MAX;
    }

    pub(super) const MAX_UNDO_HISTORY: usize = 100;

    pub(super) fn record_input_undo_snapshot(&mut self) {
        let snapshot = self.current_input_snapshot();
        if self.input_undo_stack.last() != Some(&snapshot) {
            self.input_undo_stack.push(snapshot);
            if self.input_undo_stack.len() > Self::MAX_UNDO_HISTORY {
                self.input_undo_stack.remove(0);
            }
        }
        self.input_redo_stack.clear();
    }

    pub(super) fn clear_input_history(&mut self) {
        self.input_undo_stack.clear();
        self.input_redo_stack.clear();
    }

    /// Reset all input editor fields atomically. Ensures `text`, cursors,
    /// `pastes`, and `next_paste_id` always move together so no stale mask
    /// entries survive a prompt boundary.
    pub(super) fn reset_input(&mut self) {
        self.input.text.clear();
        self.input.cursor = 0;
        self.input.byte_cursor = 0;
        self.input.preferred_col = None;
        self.input.pastes.clear();
        self.input.next_paste_id = 1;
    }

    /// Ctrl-W: delete backward to the previous word boundary, treating each
    /// paste mask as a single atomic unit (stops at the mask boundary rather
    /// than scanning inside it).
    pub(super) fn word_backspace(&mut self) {
        self.input_scroll_manual = false;
        self.clamp_input_cursor();
        if self.input.cursor == 0 {
            return;
        }
        self.record_input_undo_snapshot();
        if self.delete_selection_range() {
            return;
        }
        let ranges = self.compute_mask_ranges();
        // Cursor at a mask's end ￀delete the whole mask atom (same as backspace).
        if let Some((idx, r)) = ranges
            .iter()
            .find(|(_, r)| r.end == self.input.byte_cursor)
            .map(|(i, r)| (*i, r.clone()))
        {
            let del_end =
                if self.input.text.len() > r.end && self.input.text.as_bytes()[r.end] == b' ' {
                    r.end + 1
                } else {
                    r.end
                };
            self.input.text.replace_range(r.start..del_end, "");
            self.input.pastes.remove(idx);
            self.input.byte_cursor = r.start;
            self.input.cursor = self.input.text[..r.start].chars().count();
            self.input.preferred_col = None;
            self.input_scroll_offset = usize::MAX;
            return;
        }
        // Walk backward: skip trailing whitespace, then skip word chars.
        // Stop at any mask-end boundary so masks are deleted atomically.
        let bc = self.input.byte_cursor;
        let chars_before: Vec<(usize, char)> = self.input.text[..bc].char_indices().collect();
        let mut i = chars_before.len();
        while i > 0 && chars_before[i - 1].1.is_whitespace() {
            i -= 1;
        }
        while i > 0 {
            let (byte_pos, ch) = chars_before[i - 1];
            if ch.is_whitespace() {
                break;
            }
            if ranges.iter().any(|(_, r)| r.end == byte_pos) {
                break;
            }
            i -= 1;
        }
        let del_start = if i < chars_before.len() {
            chars_before[i].0
        } else {
            bc
        };
        if del_start == bc {
            return;
        }
        self.input.text.replace_range(del_start..bc, "");
        self.input.byte_cursor = del_start;
        self.input.cursor = self.input.text[..del_start].chars().count();
        self.input.preferred_col = None;
        self.input_scroll_offset = usize::MAX;
    }

    pub(super) fn undo_input_edit(&mut self) -> bool {
        let Some(snapshot) = self.input_undo_stack.pop() else {
            return false;
        };
        self.input_redo_stack.push(self.current_input_snapshot());
        self.apply_input_snapshot(snapshot);
        true
    }

    pub(super) fn redo_input_edit(&mut self) -> bool {
        let Some(snapshot) = self.input_redo_stack.pop() else {
            return false;
        };
        self.input_undo_stack.push(self.current_input_snapshot());
        self.apply_input_snapshot(snapshot);
        true
    }

    pub(super) fn input_char_len(&self) -> usize {
        self.input.text.chars().count()
    }

    /// Re-sync `byte_cursor` from `cursor` when the cursor is set directly
    /// (clamp / `set_input_cursor_line_col`).
    pub(super) fn resync_byte_cursor(&mut self) {
        self.input.byte_cursor = self
            .input
            .text
            .char_indices()
            .nth(self.input.cursor)
            .map_or(self.input.text.len(), |(idx, _)| idx);
    }

    /// Returns the byte offset of character index `char_idx` by scanning the
    /// string.  Hot-path mutators (`insert_input_char`, `insert_input_str`,
    /// etc.) use the cached `byte_cursor` field directly instead.
    pub(super) fn input_char_to_byte(&self, char_idx: usize) -> usize {
        self.input
            .text
            .char_indices()
            .nth(char_idx)
            .map_or(self.input.text.len(), |(idx, _)| idx)
    }

    pub(super) fn clamp_input_cursor(&mut self) {
        let old = self.input.cursor;
        self.input.cursor = self.input.cursor.min(self.input_char_len());
        if self.input.cursor != old {
            self.resync_byte_cursor();
        }
    }

    /// If an input selection is active, delete the selected range, move the
    /// cursor to the anchor, and clear the selection.  Returns `true` if a
    /// selection was deleted.
    pub(super) fn delete_selection_range(&mut self) -> bool {
        if let Some((a, b)) = self.input_selection.take() {
            self.input_click_anchor = None;
            self.input.cursor = a;
            self.resync_byte_cursor();
            let end_byte = self.input_char_to_byte(b);
            self.input
                .text
                .replace_range(self.input.byte_cursor..end_byte, "");
            self.input.preferred_col = None;
            self.input_scroll_offset = usize::MAX;
            true
        } else {
            false
        }
    }

    pub(super) fn selected_input_text(&self) -> Option<&str> {
        let (a, b) = self.input_selection?;
        let sel_start = self.input_char_to_byte(a);
        let sel_end = self.input_char_to_byte(b);
        self.input.text.get(sel_start..sel_end)
    }

    /// Returns the currently selected input slice with paste masks expanded back
    /// to their original content.  Used by clipboard copy/cut so the OS clipboard
    /// receives real text, not the placeholder.
    pub(super) fn selected_input_text_expanded(&self) -> Option<String> {
        let raw = self.selected_input_text()?.to_string();
        Some(expand_masks(&raw, &self.input.pastes))
    }

    pub(super) fn cut_input_selection_text(&mut self) -> Option<String> {
        let text = self.selected_input_text_expanded()?;
        self.record_input_undo_snapshot();
        self.delete_selection_range();
        Some(text)
    }

    pub(super) fn insert_input_char(&mut self, ch: char) {
        self.input_scroll_manual = false;
        self.record_input_undo_snapshot();
        self.delete_selection_range();
        self.clamp_input_cursor();
        self.input.text.insert(self.input.byte_cursor, ch);
        self.input.cursor = self.input.cursor.saturating_add(1);
        self.input.byte_cursor = self.input.byte_cursor.saturating_add(ch.len_utf8());
        self.input.preferred_col = None;
        self.input_scroll_offset = usize::MAX;
    }

    pub(super) fn insert_input_str(&mut self, text: &str) {
        self.input_scroll_manual = false;
        if text.is_empty() {
            return;
        }
        self.record_input_undo_snapshot();
        self.delete_selection_range();
        self.clamp_input_cursor();
        self.input.text.insert_str(self.input.byte_cursor, text);
        let char_count = text.chars().count();
        self.input.cursor = self.input.cursor.saturating_add(char_count);
        self.input.byte_cursor = self.input.byte_cursor.saturating_add(text.len());
        self.input.preferred_col = None;
        self.input_scroll_offset = usize::MAX;
    }

    /// Insert a paste as an atomic masked token.
    ///
    /// The pasted text has already been normalised by the caller.  This method:
    ///   1. Records an undo snapshot.
    ///   2. Generates a `[#N Pasted ~K lines]` placeholder and appends a trailing
    ///      space so subsequent typing doesn't visually fuse with the mask.
    ///   3. Stores the original content in `pastes` for later expansion on submit
    ///      or clipboard yank.
    pub(super) fn insert_paste_mask(&mut self, raw: &str) {
        self.input_scroll_manual = false;
        if raw.is_empty() {
            return;
        }
        self.record_input_undo_snapshot();
        self.delete_selection_range();
        self.clamp_input_cursor();

        // Edge case: if cursor is strictly inside any existing mask, snap to the
        // nearer boundary before inserting.  Prevents the placeholder from being
        // inserted mid-placeholder which would break later text.find lookups for
        // the first mask.
        let ranges = self.compute_mask_ranges();
        if let Some(r) = Self::mask_containing(self.input.byte_cursor, &ranges) {
            let snap_to = if self.input.byte_cursor - r.start < r.end - self.input.byte_cursor {
                r.start
            } else {
                r.end
            };
            self.input.byte_cursor = snap_to;
            self.input.cursor = self.input.text[..snap_to].chars().count();
        }

        let id = self.input.next_paste_id;
        self.input.next_paste_id += 1;
        let placeholder = format_paste_placeholder(id, count_lines(raw));
        let to_insert = format!("{placeholder} ");

        self.input
            .text
            .insert_str(self.input.byte_cursor, &to_insert);
        let char_count = to_insert.chars().count();
        self.input.cursor = self.input.cursor.saturating_add(char_count);
        self.input.byte_cursor = self.input.byte_cursor.saturating_add(to_insert.len());
        self.input.preferred_col = None;
        self.input_scroll_offset = usize::MAX;

        self.input.pastes.push(PasteEntry {
            id,
            placeholder,
            content: raw.to_string(),
        });
    }

    /// Single entry point for any pasted text, regardless of source (bracketed
    /// paste, Ctrl+V, Shift+Insert, burst flush).  Normalises newlines and
    /// either masks the paste (>= threshold) or inserts it raw.
    ///
    /// Note: this does NOT set `suppress_paste_until` ￀that suppression is
    /// only appropriate for the bracketed-paste / Ctrl+V paths (where stray
    /// Enter events from the terminal can follow a paste).  The burst-flush
    /// path manages newlines via its own in-burst check on `KeyCode::Enter`,
    /// and adding suppression there would eat subsequent paste characters.
    /// Callers that need post-paste suppression set it themselves.
    pub(super) fn handle_paste_event(&mut self, raw: &str) {
        let normalised = normalize_pasted_text(raw);
        if should_mask_paste(&normalised) {
            self.insert_paste_mask(&normalised);
        } else {
            self.insert_input_str(&normalised);
        }
    }

    /// Arm the post-paste suppression window used by the bracketed-paste and
    /// Ctrl+V paths.  Stray `KeyCode::Enter` events that some terminals emit
    /// for each `\n` in a paste are discarded for the next 100 ms so they
    /// don't trigger an accidental send.
    pub(super) fn arm_paste_enter_suppression(&mut self) {
        self.selection.suppress_paste_until = Some(Instant::now() + Duration::from_millis(100));
    }

    /// Maximum gap (ms) between consecutive keystrokes considered a paste burst.
    /// Human typing at 150 WPM averages ￀80 ms/char; 30 ms is well below
    /// what any human can sustain.
    pub(super) const PASTE_BURST_THRESHOLD_MS: u64 = 30;

    /// True if the previous key event arrived within the paste-burst threshold.
    pub(super) fn in_paste_burst(&self, now: Instant) -> bool {
        self.last_key_time.is_some_and(|t| {
            now.duration_since(t) <= Duration::from_millis(Self::PASTE_BURST_THRESHOLD_MS)
        })
    }

    /// If the burst accumulator has any chars, drain it into the input via
    /// `handle_paste_event` (which applies masking and newline suppression).
    /// Called when the burst ends, or before any non-burst-compatible key.
    pub(super) fn flush_paste_burst(&mut self) {
        if self.paste_burst_chars.is_empty() {
            return;
        }
        let chars = std::mem::take(&mut self.paste_burst_chars);
        let text: String = chars.iter().collect();
        self.handle_paste_event(&text);
    }

    /// Returns true if `key_code` should be suppressed because a paste that
    /// contained newlines was processed recently.  Used by the event loop and
    /// by tests to verify suppression without running the full event loop.
    pub(super) fn paste_enter_is_suppressed(&self, key_code: KeyCode) -> bool {
        self.selection
            .suppress_paste_until
            .is_some_and(|deadline| Instant::now() <= deadline)
            && matches!(
                key_code,
                KeyCode::Char(_) | KeyCode::Enter | KeyCode::Tab | KeyCode::Backspace
            )
    }

    /// Locate every active paste mask's current byte range in `self.input.text`.
    ///
    /// Returns `(paste_index, byte_range)` pairs sorted by `byte_range.start`.
    /// Because placeholders include a unique per-prompt `#N` index, `str::find`
    /// returns at most one position per placeholder; no overlap handling needed.
    /// Orphaned entries (placeholder absent from text) are silently skipped.
    pub(super) fn compute_mask_ranges(&self) -> Vec<(usize, std::ops::Range<usize>)> {
        let mut out: Vec<(usize, std::ops::Range<usize>)> = self
            .input
            .pastes
            .iter()
            .enumerate()
            .filter_map(|(i, p)| {
                self.input
                    .text
                    .find(&p.placeholder)
                    .map(|s| (i, s..s + p.placeholder.len()))
            })
            .collect();
        out.sort_by_key(|(_, r)| r.start);
        out
    }

    /// Returns mask ranges in CHAR-index space.  Used by the input renderer to
    /// style placeholder text (`[#N Pasted ~K lines]`) with a meta-token
    /// modifier (dim + italic) so it visually reads as a token rather than
    /// user-typed content.  Sorted by `start` (matches `compute_mask_ranges`).
    pub(super) fn compute_mask_char_ranges(&self) -> Vec<(usize, usize)> {
        let byte_ranges = self.compute_mask_ranges();
        if byte_ranges.is_empty() {
            return Vec::new();
        }
        // Build a sorted (byte_offset, char_index) table in one pass; then use
        // binary search per range ￀O(n) build, O(log n) per lookup.
        let text = &self.input.text;
        let mut byte_to_char: Vec<(usize, usize)> = text
            .char_indices()
            .enumerate()
            .map(|(ci, (bi, _))| (bi, ci))
            .collect();
        let total_chars = byte_to_char.len();
        byte_to_char.push((text.len(), total_chars));

        let lookup = |byte_pos: usize| -> usize {
            let idx = byte_to_char.partition_point(|(bi, _)| *bi < byte_pos);
            byte_to_char.get(idx).map_or(total_chars, |(_, ci)| *ci)
        };

        byte_ranges
            .iter()
            .map(|(_, r)| (lookup(r.start), lookup(r.end)))
            .collect()
    }

    /// Returns the mask range that strictly contains `byte_pos`, or `None` if the
    /// position is at a boundary or outside any mask.  Boundaries are valid cursor
    /// positions; only interior positions need to be snapped.
    pub(super) fn mask_containing(
        byte_pos: usize,
        ranges: &[(usize, std::ops::Range<usize>)],
    ) -> Option<std::ops::Range<usize>> {
        ranges
            .iter()
            .find(|(_, r)| byte_pos > r.start && byte_pos < r.end)
            .map(|(_, r)| r.clone())
    }

    pub(super) fn backspace_input_char(&mut self) {
        self.input_scroll_manual = false;
        let had_selection = self.input_selection.is_some();
        self.clamp_input_cursor();
        if !had_selection && self.input.cursor == 0 {
            return;
        }
        self.record_input_undo_snapshot();
        // If selection is active, delete it instead of a single char.
        if self.delete_selection_range() {
            return;
        }
        // Atomic-mask deletion: cursor at a mask's end byte -> drop whole mask
        // plus its trailing separator space.
        if !had_selection {
            let ranges = self.compute_mask_ranges();
            if let Some((idx, r)) = ranges
                .iter()
                .find(|(_, r)| r.end == self.input.byte_cursor)
                .map(|(i, r)| (*i, r.clone()))
            {
                let del_end =
                    if self.input.text.len() > r.end && self.input.text.as_bytes()[r.end] == b' ' {
                        r.end + 1
                    } else {
                        r.end
                    };
                self.input.text.replace_range(r.start..del_end, "");
                self.input.pastes.remove(idx);
                self.input.byte_cursor = r.start;
                self.input.cursor = self.input.text[..r.start].chars().count();
                self.input.preferred_col = None;
                self.input_scroll_offset = usize::MAX;
                return;
            }
        }
        // Find byte-offset of the character before cursor
        let prev_byte = if self.input.byte_cursor > 0 {
            // Walk backwards from byte_cursor to the previous char boundary
            let bytes = self.input.text.as_bytes();
            let mut pos = self.input.byte_cursor - 1;
            while pos > 0 && (bytes[pos] & 0xC0) == 0x80 {
                pos -= 1;
            }
            pos
        } else {
            0
        };
        let start = prev_byte;
        let end = self.input.byte_cursor;
        self.input.text.replace_range(start..end, "");
        self.input.cursor -= 1;
        self.input.byte_cursor = start;
        self.input.preferred_col = None;
        self.input_scroll_offset = usize::MAX;
    }

    pub(super) fn delete_input_char(&mut self) {
        self.input_scroll_manual = false;
        let had_selection = self.input_selection.is_some();
        self.clamp_input_cursor();
        if !had_selection && self.input.cursor >= self.input_char_len() {
            return;
        }
        self.record_input_undo_snapshot();
        if self.delete_selection_range() {
            return;
        }
        // Atomic-mask deletion: cursor at a mask's start byte -> drop whole mask
        // plus its trailing separator space.
        if !had_selection {
            let ranges = self.compute_mask_ranges();
            if let Some((idx, r)) = ranges
                .iter()
                .find(|(_, r)| r.start == self.input.byte_cursor)
                .map(|(i, r)| (*i, r.clone()))
            {
                let del_end =
                    if self.input.text.len() > r.end && self.input.text.as_bytes()[r.end] == b' ' {
                        r.end + 1
                    } else {
                        r.end
                    };
                self.input.text.replace_range(r.start..del_end, "");
                self.input.pastes.remove(idx);
                // byte_cursor stays at r.start; char cursor stays the same.
                self.input.preferred_col = None;
                self.input_scroll_offset = usize::MAX;
                return;
            }
        }
        // Find byte-offset of the character after cursor
        let bytes = self.input.text.as_bytes();
        let mut end = self.input.byte_cursor + 1;
        while end < bytes.len() && (bytes[end] & 0xC0) == 0x80 {
            end += 1;
        }
        self.input
            .text
            .replace_range(self.input.byte_cursor..end, "");
        self.input.preferred_col = None;
        self.input_scroll_offset = usize::MAX;
    }

    pub(super) fn input_cursor_line_col(&self) -> (usize, usize) {
        let mut line = 0usize;
        let mut col = 0usize;
        for (idx, ch) in self.input.text.chars().enumerate() {
            if idx == self.input.cursor {
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

    pub(super) fn input_lines(&self) -> Vec<&str> {
        self.input.text.split('\n').collect()
    }

    pub(super) fn set_input_cursor_line_col(&mut self, target_line: usize, target_col: usize) {
        self.input_scroll_manual = false;
        let lines = self.input_lines();
        let line = target_line.min(lines.len().saturating_sub(1));
        let col = char_count_for_display_col(lines[line], target_col);
        let mut cursor = 0usize;
        for input_line in lines.iter().take(line) {
            cursor += input_line.chars().count() + 1;
        }
        cursor += col;
        self.input.cursor = cursor.min(self.input_char_len());
        self.resync_byte_cursor();
        self.input_scroll_offset = usize::MAX;
    }

    pub(super) fn move_input_cursor_left(&mut self) {
        self.input_scroll_manual = false;
        self.input_selection = None;
        self.input_click_anchor = None;
        if self.input.cursor == 0 {
            return;
        }
        // Walk backwards from byte_cursor to the previous char boundary
        let bytes = self.input.text.as_bytes();
        let mut pos = self.input.byte_cursor.saturating_sub(1);
        while pos > 0 && (bytes[pos] & 0xC0) == 0x80 {
            pos -= 1;
        }
        self.input.byte_cursor = pos;
        self.input.cursor -= 1;
        self.input.preferred_col = None;
        self.input_scroll_offset = usize::MAX;
        // Atomic-mask snap: if we landed strictly inside any mask, jump to its start.
        let ranges = self.compute_mask_ranges();
        if let Some(r) = Self::mask_containing(self.input.byte_cursor, &ranges) {
            self.input.byte_cursor = r.start;
            self.input.cursor = self.input.text[..r.start].chars().count();
        }
    }

    pub(super) fn move_input_cursor_right(&mut self) {
        self.input_scroll_manual = false;
        self.input_selection = None;
        self.input_click_anchor = None;
        if self.input.cursor >= self.input_char_len() {
            return;
        }
        // Walk forward from byte_cursor past the current character
        let bytes = self.input.text.as_bytes();
        let mut end = self.input.byte_cursor + 1;
        while end < bytes.len() && (bytes[end] & 0xC0) == 0x80 {
            end += 1;
        }
        self.input.byte_cursor = end;
        self.input.cursor += 1;
        self.input.preferred_col = None;
        self.input_scroll_offset = usize::MAX;
        // Atomic-mask snap: if we landed strictly inside any mask, jump to its end.
        let ranges = self.compute_mask_ranges();
        if let Some(r) = Self::mask_containing(self.input.byte_cursor, &ranges) {
            self.input.byte_cursor = r.end;
            self.input.cursor = self.input.text[..r.end].chars().count();
        }
    }

    pub(super) fn move_input_cursor_home(&mut self) {
        let (line, _) = self.input_cursor_line_col();
        self.set_input_cursor_line_col(line, 0);
        self.input.preferred_col = Some(0);
    }

    pub(super) fn move_input_cursor_end(&mut self) {
        let (line, _) = self.input_cursor_line_col();
        let target = self
            .input_lines()
            .get(line)
            .map_or(0, |input_line| text_display_width(input_line));
        self.set_input_cursor_line_col(line, target);
        self.input.preferred_col = Some(target);
    }

    pub(super) fn select_all_input(&mut self) {
        let char_len = self.input_char_len();
        if char_len == 0 {
            return;
        }
        self.input_scroll_manual = false;
        self.input_selection = Some((0, char_len));
        self.input_click_anchor = None;
        self.input.cursor = char_len;
        self.resync_byte_cursor();
        self.input.preferred_col = None;
        self.input_scroll_offset = usize::MAX;
    }

    /// Compute visual line boundaries for the input text.
    ///
    /// Returns `Vec<(char_start, display_width, starts_paragraph)>`.
    ///
    /// - `char_start`: char-index of the first character on this visual line.
    /// - `display_width`: width of the visual line in terminal cells.
    /// - `starts_paragraph`: `true` iff this is the first visual line of a
    ///   logical paragraph after a `\n` separator (i.e. the char immediately
    ///   before `char_start` in the text is `\n`). Always `false` for the
    ///   very first visual line and for soft-wrapped continuation lines.
    ///
    /// Callers use `starts_paragraph` instead of re-scanning the text with
    /// `chars().nth()` to check for a trailing `\n` at a line boundary.
    pub(super) fn visual_line_info(&self, safe_width: usize) -> Vec<(usize, usize, bool)> {
        let mut lines = Vec::new();
        let mut char_idx = 0usize;
        let parts: Vec<&str> = self.input.text.split('\n').collect();
        for (logical_idx, logical_line) in parts.iter().enumerate() {
            let logical_chars: Vec<char> = logical_line.chars().collect();
            let prompt_offset = if logical_idx == 0 { 2usize } else { 0 };
            let first_cap = safe_width.saturating_sub(prompt_offset);
            let cap = safe_width;
            let mut offset = 0usize;
            loop {
                let remaining = logical_chars.len().saturating_sub(offset);
                if remaining == 0 {
                    if offset == 0 {
                        // First visual line of a new paragraph starts after \n
                        // (except the very first logical line which has no preceding \n).
                        lines.push((char_idx, 0, logical_idx > 0));
                    }
                    break;
                }
                let w = if offset == 0 { first_cap } else { cap };
                // Walk forward up to `w` display cells
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
                // starts_paragraph: first soft-wrap line of a new logical paragraph
                let starts_paragraph = offset == 0 && logical_idx > 0;
                lines.push((char_idx + offset, col, starts_paragraph));
                if end == offset {
                    // Zero-width char at start ￀force advance
                    end = offset + 1;
                }
                offset = end;
            }
            char_idx += logical_chars.len();
            // Only add 1 for the newline separator if this is not the last part
            if logical_idx < parts.len() - 1 {
                char_idx += 1;
            }
        }
        lines
    }

    pub(super) fn move_input_cursor_up(&mut self) {
        self.move_input_cursor_up_inner();
        // Atomic-mask snap: up = treat like left = snap to start.
        let ranges = self.compute_mask_ranges();
        if let Some(r) = Self::mask_containing(self.input.byte_cursor, &ranges) {
            self.input.byte_cursor = r.start;
            self.input.cursor = self.input.text[..r.start].chars().count();
        }
    }

    pub(super) fn move_input_cursor_up_inner(&mut self) {
        // Fall back to logical-line navigation until the widget width is known
        // (first render hasn't happened yet).
        if self.input_area_width == 0 {
            self.move_input_cursor_up_logical();
            return;
        }
        let w = usize::from(self.input_area_width).max(10);
        let vis = self.visual_line_info(w.saturating_sub(5).max(5));
        if vis.is_empty() {
            return;
        }
        let cur_char = self.input.cursor;
        // Find which visual line the cursor is on.
        // partition_point gives the last line whose start <= cur_char.
        let cur_vis = vis
            .partition_point(|(start, _, _)| *start <= cur_char)
            .saturating_sub(1);
        if cur_vis == 0 {
            let (start, _, _) = vis[0];
            self.set_input_cursor_line_col_by_char(start);
            self.input.preferred_col = self.input.preferred_col.or(Some(0));
            return;
        }
        let cur_start = vis[cur_vis].0;
        let cur_end = vis.get(cur_vis + 1).map_or(self.input_char_len(), |v| v.0);
        let cur_offset = cur_char.saturating_sub(cur_start);
        let clamped_offset = cur_offset.min(cur_end.saturating_sub(cur_start));
        let cur_display_col = self
            .input
            .text
            .chars()
            .skip(cur_start)
            .take(clamped_offset)
            .map(char_display_width)
            .sum::<usize>();
        let preferred_col = self.input.preferred_col.unwrap_or(cur_display_col);

        let (prev_start, prev_width, _) = vis[cur_vis - 1];
        // vis[cur_vis].2 is true when cur_vis starts a new logical paragraph,
        // meaning the char before it is a \n separator to exclude from the range.
        let raw_prev_end = vis[cur_vis].0;
        let prev_end = if vis[cur_vis].2 {
            raw_prev_end.saturating_sub(1)
        } else {
            raw_prev_end
        };
        if prev_width == 0 {
            self.set_input_cursor_line_col_by_char(prev_start);
            self.input.preferred_col = Some(preferred_col);
            return;
        }
        let target_col = preferred_col.min(prev_width);
        let prev_text: String = self
            .input
            .text
            .chars()
            .skip(prev_start)
            .take(prev_end.saturating_sub(prev_start))
            .collect();
        let col_chars = char_count_for_display_col(&prev_text, target_col);
        self.set_input_cursor_line_col_by_char(prev_start + col_chars);
        self.input.preferred_col = Some(preferred_col);
    }

    pub(super) fn move_input_cursor_down(&mut self) {
        self.move_input_cursor_down_inner();
        // Atomic-mask snap: down = treat like right = snap to end.
        let ranges = self.compute_mask_ranges();
        if let Some(r) = Self::mask_containing(self.input.byte_cursor, &ranges) {
            self.input.byte_cursor = r.end;
            self.input.cursor = self.input.text[..r.end].chars().count();
        }
    }

    pub(super) fn move_input_cursor_down_inner(&mut self) {
        // Fall back to logical-line navigation until the widget width is known.
        if self.input_area_width == 0 {
            self.move_input_cursor_down_logical();
            return;
        }
        let w = usize::from(self.input_area_width).max(10);
        let vis = self.visual_line_info(w.saturating_sub(5).max(5));
        if vis.is_empty() {
            return;
        }
        let cur_char = self.input.cursor;
        let cur_vis = vis
            .partition_point(|(start, _, _)| *start <= cur_char)
            .saturating_sub(1);
        if cur_vis + 1 >= vis.len() {
            // Already on the last visual line ￀go to end
            let (start, width, _) = vis[cur_vis];
            let line_text: String = self.input.text.chars().skip(start).collect();
            self.set_input_cursor_line_col_by_char(
                start + char_count_for_display_col(&line_text, width),
            );
            self.input.preferred_col = self.input.preferred_col.or(Some(width));
            return;
        }
        let cur_start = vis[cur_vis].0;
        let cur_end = vis[cur_vis + 1].0;
        let cur_offset = cur_char.saturating_sub(cur_start);
        let clamped_offset = cur_offset.min(cur_end.saturating_sub(cur_start));
        let cur_display_col = self
            .input
            .text
            .chars()
            .skip(cur_start)
            .take(clamped_offset)
            .map(char_display_width)
            .sum::<usize>();
        // Resolve preferred column from current position (or previous nav).
        let preferred_col = self.input.preferred_col.unwrap_or(cur_display_col);

        let (next_start, next_width, _) = vis[cur_vis + 1];
        let raw_next_end = vis.get(cur_vis + 2).map_or(self.input_char_len(), |v| v.0);
        // vis[cur_vis + 2].2 tells us whether the line after next starts a new
        // paragraph, meaning next's last char is the \n separator to exclude.
        let next_ends_with_nl = vis.get(cur_vis + 2).is_some_and(|v| v.2);
        let next_end = if next_ends_with_nl {
            raw_next_end.saturating_sub(1)
        } else {
            raw_next_end
        };
        // Empty visual line ￀land at its start; keep preferred_col.
        if next_width == 0 {
            self.set_input_cursor_line_col_by_char(next_start);
            self.input.preferred_col = Some(preferred_col);
            return;
        }
        let target_col = preferred_col.min(next_width);
        let next_text: String = self
            .input
            .text
            .chars()
            .skip(next_start)
            .take(next_end.saturating_sub(next_start))
            .collect();
        let col_chars = char_count_for_display_col(&next_text, target_col);
        self.set_input_cursor_line_col_by_char(next_start + col_chars);
        self.input.preferred_col = Some(preferred_col);
    }

    /// Set cursor directly by character index (used by visual-line nav).
    pub(super) fn set_input_cursor_line_col_by_char(&mut self, char_idx: usize) {
        self.input_scroll_manual = false;
        self.input_selection = None;
        self.input_click_anchor = None;
        self.input.cursor = char_idx.min(self.input_char_len());
        self.resync_byte_cursor();
        self.input_scroll_offset = usize::MAX;
    }

    /// Convert a mouse position (row, col) relative to the input widget into
    /// a character index in `input.text`.  Returns `None` if the position
    /// falls outside the text bounds.
    pub(super) fn char_index_at_mouse(
        &self,
        widget_row: usize,
        widget_col: usize,
    ) -> Option<usize> {
        if widget_row == 0 {
            return None;
        }
        let w = usize::from(self.input_area_width).max(10);
        let safe_width = w.saturating_sub(5).max(5);
        let vis = self.visual_line_info(safe_width);
        let content_row = widget_row.saturating_sub(1);
        let abs_row = self.input_scroll_offset + content_row;
        let &(start, width, _) = vis.get(abs_row)?;
        let is_first = abs_row == 0;
        let prompt = if is_first { 2usize } else { 0 };
        if widget_col < prompt {
            return Some(start);
        }
        let target_col = widget_col.saturating_sub(prompt).min(width);
        // Compute the char end of this visual line.  The next visual line's
        // start gives the exclusive end; subtract 1 if it starts a new paragraph
        // (meaning the intervening char is a \n separator not part of either line).
        let raw_end = vis.get(abs_row + 1).map_or(self.input_char_len(), |v| v.0);
        let next_starts_paragraph = vis.get(abs_row + 1).is_some_and(|v| v.2);
        let line_end = if next_starts_paragraph {
            raw_end.saturating_sub(1)
        } else {
            raw_end
        };
        let mut char_idx = start;
        let mut col = 0usize;
        for ch in self
            .input
            .text
            .chars()
            .skip(start)
            .take(line_end.saturating_sub(start))
        {
            let cw = char_display_width(ch);
            if cw == 0 {
                continue;
            }
            if col + cw > target_col {
                break;
            }
            col += cw;
            char_idx += 1;
        }
        Some(char_idx)
    }

    /// Logical-line up/down (jumps by `\n`, ignoring soft wraps).  Used as a
    /// fallback before the widget width is known, and for Home/End which
    /// operate on logical lines.
    pub(super) fn move_input_cursor_up_logical(&mut self) {
        let (line, col) = self.input_cursor_line_col();
        if line == 0 {
            self.set_input_cursor_line_col(0, 0);
            self.input.preferred_col = Some(0);
            return;
        }
        let target_col = self.input.preferred_col.unwrap_or(col);
        self.set_input_cursor_line_col(line - 1, target_col);
        self.input.preferred_col = Some(target_col);
    }

    pub(super) fn move_input_cursor_down_logical(&mut self) {
        let line_widths = self
            .input_lines()
            .into_iter()
            .map(text_display_width)
            .collect::<Vec<_>>();
        let (line, col) = self.input_cursor_line_col();
        if line + 1 >= line_widths.len() {
            self.set_input_cursor_line_col(line, line_widths[line]);
            self.input.preferred_col = Some(line_widths[line]);
            return;
        }
        let target_col = self.input.preferred_col.unwrap_or(col);
        self.set_input_cursor_line_col(line + 1, target_col);
        self.input.preferred_col = Some(target_col);
    }

    pub(super) fn handle_tool_call_start(&mut self, name: String, input: &str) {
        let input_summary = tool_input_summary(&name, input);
        self.ui_state = AppUiState::ChatMode;
        self.current_tool = Some(name.clone());
        self.live_tool_calls
            .push((name, input_summary, ToolCallStatus::Running));
    }

    pub(super) fn handle_tool_call_complete(&mut self, name: &str, output: String, is_error: bool) {
        let status = if is_error {
            ToolCallStatus::Error(output)
        } else {
            ToolCallStatus::Success { output }
        };

        // Find the first Running entry with a matching tool name.
        // This correctly handles multiple parallel calls of the same tool
        // (e.g. two navigate calls in one assistant turn) because completions
        // arrive in the same order the calls were started, and each completion
        // consumes exactly the first still-Running entry.
        if let Some((_, _, entry_status)) =
            self.live_tool_calls
                .iter_mut()
                .rev()
                .find(|(entry_name, _, entry_status)| {
                    entry_name == name && matches!(entry_status, ToolCallStatus::Running)
                })
        {
            *entry_status = status;
        }
    }
}
