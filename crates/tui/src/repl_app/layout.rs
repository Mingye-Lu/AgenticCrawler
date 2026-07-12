use super::*;

#[allow(clippy::too_many_lines)]
/// Push spans for `text` whose characters start at absolute char index
/// `text_char_start` within `self.input.text`, applying `mask_style` for
/// any chars overlapping a mask range and `base_style` otherwise.  Mask
/// ranges are absolute (input-text char indices) and assumed sorted.
pub(super) fn push_input_text_spans(
    spans: &mut Vec<Span<'static>>,
    text: &str,
    text_char_start: usize,
    base_style: Style,
    mask_style: Style,
    masks: &[(usize, usize)],
) {
    if text.is_empty() {
        return;
    }
    if masks.is_empty() {
        spans.push(Span::styled(text.to_string(), base_style));
        return;
    }
    let chars: Vec<char> = text.chars().collect();
    let text_char_end = text_char_start + chars.len();
    // Find masks that overlap [text_char_start, text_char_end).
    let mut cursor = 0usize; // char index within `text`
    for &(m_start, m_end) in masks {
        if m_end <= text_char_start || m_start >= text_char_end {
            continue;
        }
        let local_start = m_start.saturating_sub(text_char_start);
        let local_end = (m_end - text_char_start).min(chars.len());
        if local_start > cursor {
            let pre: String = chars[cursor..local_start].iter().collect();
            spans.push(Span::styled(pre, base_style));
        }
        if local_end > local_start {
            let inside: String = chars[local_start..local_end].iter().collect();
            spans.push(Span::styled(inside, mask_style));
        }
        cursor = local_end;
    }
    if cursor < chars.len() {
        let tail: String = chars[cursor..].iter().collect();
        spans.push(Span::styled(tail, base_style));
    }
}

impl ReplTuiState {
    #[allow(clippy::too_many_lines)]
    pub(crate) fn calculate_input_dimensions(
        &mut self,
        width: u16,
        model_label: &str,
    ) -> (u16, Vec<Line<'static>>, usize, Option<(u16, u16)>) {
        self.input_area_width = width;
        self.clamp_input_cursor();
        let is_placeholder = self.input.text.is_empty();
        let placeholder_text = self.input_placeholder();
        let mut input_with_caret = self.input.text.clone();
        if !is_placeholder {
            let caret_idx = self.input_char_to_byte(self.input.cursor);
            input_with_caret.insert(caret_idx, INPUT_CARET_MARKER);
        }
        let source = if is_placeholder {
            placeholder_text.to_owned()
        } else {
            input_with_caret
        };
        let mut lines_data = source
            .split('\n')
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();
        if lines_data.is_empty() {
            lines_data.push(String::new());
        }

        let safe_width = width.saturating_sub(5).max(5) as usize;
        let mut visual_lines = Vec::new();
        let mut caret_row_idx = 0usize;
        let mut seen_caret = false;

        let input_char_width = |ch: char| {
            if ch == INPUT_CARET_MARKER {
                0
            } else {
                char_display_width(ch)
            }
        };

        for (logical_idx, line) in lines_data.into_iter().enumerate() {
            let offset = if logical_idx == 0 { 2 } else { 0 };
            let first_line_width = safe_width.saturating_sub(offset);

            if line.is_empty() {
                visual_lines.push((logical_idx == 0, String::new()));
                continue;
            }

            let mut current = String::new();
            let mut w = 0;
            let mut is_first_chunk = true;

            for c in line.chars() {
                let char_width = input_char_width(c);
                let target = if is_first_chunk {
                    first_line_width
                } else {
                    safe_width
                };
                if !current.is_empty() && w + char_width > target {
                    if !is_placeholder && !seen_caret && current.contains(INPUT_CARET_MARKER) {
                        caret_row_idx = visual_lines.len();
                        seen_caret = true;
                    }
                    visual_lines.push((logical_idx == 0 && is_first_chunk, current));
                    current = String::new();
                    w = 0;
                    is_first_chunk = false;
                }

                current.push(c);
                w += char_width;

                if w >= target && !current.is_empty() {
                    if !is_placeholder && !seen_caret && current.contains(INPUT_CARET_MARKER) {
                        caret_row_idx = visual_lines.len();
                        seen_caret = true;
                    }
                    visual_lines.push((logical_idx == 0 && is_first_chunk, current));
                    current = String::new();
                    w = 0;
                    is_first_chunk = false;
                }
            }
            if !current.is_empty() {
                if !is_placeholder && !seen_caret && current.contains(INPUT_CARET_MARKER) {
                    caret_row_idx = visual_lines.len();
                    seen_caret = true;
                }
                visual_lines.push((logical_idx == 0 && is_first_chunk, current));
            }
        }

        let max_text_lines = MAX_INPUT_LINES;
        let total_visual = visual_lines.len();
        let max_scroll = total_visual.saturating_sub(max_text_lines);
        if self.input_scroll_offset == usize::MAX {
            self.input_scroll_offset = max_scroll;
        } else {
            self.input_scroll_offset = self.input_scroll_offset.clamp(0, max_scroll);
        }
        if !is_placeholder && seen_caret && !self.input_scroll_manual {
            if caret_row_idx < self.input_scroll_offset {
                self.input_scroll_offset = caret_row_idx;
            } else if caret_row_idx >= self.input_scroll_offset + max_text_lines {
                self.input_scroll_offset = caret_row_idx.saturating_sub(max_text_lines - 1);
            }
        }
        self.input_scroll_offset = self.input_scroll_offset.clamp(0, max_scroll);

        // Compute char-index range for each visual line using the original
        // input text so embedded `\n` separators keep their real char indices.
        let visual_ranges: Vec<(usize, usize)> = if is_placeholder {
            vec![(0, 0); visual_lines.len()]
        } else {
            let vis = self.visual_line_info(safe_width);
            vis.iter()
                .enumerate()
                .map(|(idx, &(start, _, _))| {
                    let raw_end = vis.get(idx + 1).map_or(self.input_char_len(), |v| v.0);
                    let next_starts_paragraph = vis.get(idx + 1).is_some_and(|v| v.2);
                    let line_end = if next_starts_paragraph {
                        raw_end.saturating_sub(1)
                    } else {
                        raw_end
                    };
                    (start, line_end)
                })
                .collect()
        };

        let skip = self.input_scroll_offset;
        let sliced = visual_lines
            .into_iter()
            .skip(skip)
            .take(max_text_lines)
            .collect::<Vec<_>>();
        let total_sliced = sliced.len();
        let mut cursor_pos: Option<(u16, u16)> = None;

        let text_style = if is_placeholder {
            Style::default().fg(Color::DarkGray)
        } else if self.busy {
            Style::default().fg(Color::Rgb(100, 100, 100)) // Dimmed text during AI turn
        } else {
            Style::default()
        };
        let mask_style = text_style.add_modifier(Modifier::DIM | Modifier::ITALIC);
        let mask_char_ranges = if is_placeholder {
            Vec::new()
        } else {
            self.compute_mask_char_ranges()
        };

        let mut render_lines = Vec::new();
        render_lines.push(Line::from(""));

        for (i, (has_prompt, row)) in sliced.into_iter().enumerate() {
            let mut spans: Vec<Span<'static>> = Vec::new();

            if has_prompt {
                spans.push(Span::styled("❯ ", Style::default().fg(Color::LightCyan)));
            } else if skip > 0 && i == 0 {
                // If skipped first line with prompt, no visual space pad needed per standard terminal behavior
            }

            // Absolute visual-line index before skipping.
            let abs_i = skip + i;
            let (line_start, line_end) = visual_ranges.get(abs_i).copied().unwrap_or((0, 0));

            if is_placeholder && i == 0 {
                spans.push(Span::styled(row, text_style));
                let prompt_width = if has_prompt {
                    u16::try_from(text_display_width("❯ ")).unwrap_or(u16::MAX)
                } else {
                    0
                };
                cursor_pos = Some((u16::try_from(i + 1).unwrap_or(1), prompt_width));
            } else if let Some((sel_a, sel_b)) = self.input_selection {
                let sel_a = sel_a.max(line_start).min(line_end);
                let sel_b = sel_b.max(line_start).min(line_end);
                let sel_style = Style::default().fg(Color::White).bg(Color::DarkGray);
                let mut marker_idx = None;
                for (idx, ch) in row.chars().enumerate() {
                    if ch == INPUT_CARET_MARKER {
                        marker_idx = Some(idx);
                        break;
                    }
                }
                if let Some(marker_char_idx) = marker_idx {
                    let left = row.chars().take(marker_char_idx).collect::<String>();
                    let right = row.chars().skip(marker_char_idx + 1).collect::<String>();
                    // Reconstruct the full text without the marker so we can
                    // apply selection-splitting on the full visual line.
                    let full: String = left.clone() + &right;
                    let prompt_width = if has_prompt {
                        u16::try_from(text_display_width("❯ ")).unwrap_or(u16::MAX)
                    } else {
                        0
                    };
                    if sel_a < sel_b {
                        let row_sel_start = sel_a - line_start;
                        let row_sel_end = sel_b - line_start;
                        let before_s: String = full.chars().take(row_sel_start).collect();
                        let selected_s: String = full
                            .chars()
                            .skip(row_sel_start)
                            .take(row_sel_end.saturating_sub(row_sel_start))
                            .collect();
                        let after_s: String = full.chars().skip(row_sel_end).collect();
                        push_input_text_spans(
                            &mut spans,
                            &before_s,
                            line_start,
                            text_style,
                            mask_style,
                            &mask_char_ranges,
                        );
                        if !selected_s.is_empty() {
                            let sel_mask_style =
                                sel_style.add_modifier(Modifier::DIM | Modifier::ITALIC);
                            push_input_text_spans(
                                &mut spans,
                                &selected_s,
                                line_start + row_sel_start,
                                sel_style,
                                sel_mask_style,
                                &mask_char_ranges,
                            );
                        }
                        push_input_text_spans(
                            &mut spans,
                            &after_s,
                            line_start + row_sel_end,
                            text_style,
                            mask_style,
                            &mask_char_ranges,
                        );
                    } else {
                        push_input_text_spans(
                            &mut spans,
                            &left,
                            line_start,
                            text_style,
                            mask_style,
                            &mask_char_ranges,
                        );
                        push_input_text_spans(
                            &mut spans,
                            &right,
                            line_start + left.chars().count(),
                            text_style,
                            mask_style,
                            &mask_char_ranges,
                        );
                    }
                    if left.is_empty() {
                        cursor_pos = Some((u16::try_from(i + 1).unwrap_or(1), prompt_width));
                    } else {
                        let left_width = text_display_width(&left);
                        let cursor_col =
                            prompt_width + u16::try_from(left_width).unwrap_or(u16::MAX);
                        cursor_pos = Some((u16::try_from(i + 1).unwrap_or(1), cursor_col));
                    }
                } else if sel_a < sel_b {
                    let row_sel_start = sel_a - line_start;
                    let row_sel_end = sel_b - line_start;
                    let before: String = row.chars().take(row_sel_start).collect();
                    let selected: String = row
                        .chars()
                        .skip(row_sel_start)
                        .take(row_sel_end - row_sel_start)
                        .collect();
                    let after: String = row.chars().skip(row_sel_end).collect();
                    push_input_text_spans(
                        &mut spans,
                        &before,
                        line_start,
                        text_style,
                        mask_style,
                        &mask_char_ranges,
                    );
                    if !selected.is_empty() {
                        let sel_mask_style =
                            sel_style.add_modifier(Modifier::DIM | Modifier::ITALIC);
                        push_input_text_spans(
                            &mut spans,
                            &selected,
                            line_start + row_sel_start,
                            sel_style,
                            sel_mask_style,
                            &mask_char_ranges,
                        );
                    }
                    push_input_text_spans(
                        &mut spans,
                        &after,
                        line_start + row_sel_end,
                        text_style,
                        mask_style,
                        &mask_char_ranges,
                    );
                } else {
                    push_input_text_spans(
                        &mut spans,
                        &row,
                        line_start,
                        text_style,
                        mask_style,
                        &mask_char_ranges,
                    );
                }
            } else {
                // No active selection - plain rendering (existing logic).
                let mut marker_idx = None;
                for (idx, ch) in row.chars().enumerate() {
                    if ch == INPUT_CARET_MARKER {
                        marker_idx = Some(idx);
                        break;
                    }
                }
                if let Some(marker_char_idx) = marker_idx {
                    let left = row.chars().take(marker_char_idx).collect::<String>();
                    let right = row.chars().skip(marker_char_idx + 1).collect::<String>();
                    let prompt_width = if has_prompt {
                        u16::try_from(text_display_width("❯ ")).unwrap_or(u16::MAX)
                    } else {
                        0
                    };
                    let left_char_len = left.chars().count();
                    if left.is_empty() {
                        cursor_pos = Some((u16::try_from(i + 1).unwrap_or(1), prompt_width));
                    } else {
                        let left_width = text_display_width(&left);
                        push_input_text_spans(
                            &mut spans,
                            &left,
                            line_start,
                            text_style,
                            mask_style,
                            &mask_char_ranges,
                        );
                        let cursor_col =
                            prompt_width + u16::try_from(left_width).unwrap_or(u16::MAX);
                        cursor_pos = Some((u16::try_from(i + 1).unwrap_or(1), cursor_col));
                    }
                    push_input_text_spans(
                        &mut spans,
                        &right,
                        line_start + left_char_len,
                        text_style,
                        mask_style,
                        &mask_char_ranges,
                    );
                } else {
                    push_input_text_spans(
                        &mut spans,
                        &row,
                        line_start,
                        text_style,
                        mask_style,
                        &mask_char_ranges,
                    );
                }
            }
            render_lines.push(Line::from(spans));
        }

        render_lines.push(Line::from(""));
        render_lines.push(Line::from(Span::styled(
            format!("Model: {model_label}"),
            Style::default()
                .fg(Color::Rgb(128, 136, 146))
                .add_modifier(Modifier::DIM),
        )));

        #[allow(clippy::cast_possible_truncation)]
        let box_height = (total_sliced as u16) + 5;
        (box_height, render_lines, max_scroll, cursor_pos)
    }
}
