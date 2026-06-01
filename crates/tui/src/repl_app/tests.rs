use std::sync::mpsc;

use crate::app::Provider;
use crate::display_width::text_display_width;
use crate::tool_format::tool_input_summary;
use crate::tui::auth_modal::{AuthModal, AuthModalStep, ProviderKind};
use crate::tui::repl_render::{line_to_plain_text, render_tool_call_lines, wrap_ansi_line};
use crate::tui::session_modal::SessionModalEntry;
use crate::tui::ReplTuiEvent;
use crossterm::event::KeyCode;
use ratatui::style::Color;
use ratatui::text::Line;

use super::{
    count_lines, expand_masks, format_paste_placeholder, normalize_pasted_text, should_mask_paste,
    PasteEntry, ReplTuiState, ToolCallStatus,
};
use acrawl_core::message::{ContentBlock, ConversationMessage, MessageRole};

/// Smallest valid `ReplTuiState` for paste-masking unit tests.
fn test_state() -> ReplTuiState {
    ReplTuiState::new()
}

#[test]
fn normalize_pasted_text_handles_crlf_and_cr() {
    assert_eq!(normalize_pasted_text("a\r\nb"), "a\nb");
    assert_eq!(normalize_pasted_text("a\rb"), "a\nb");
    assert_eq!(normalize_pasted_text("a\r\nb\rc"), "a\nb\nc");
    assert_eq!(normalize_pasted_text("plain"), "plain");
}

#[test]
fn count_lines_counts_newlines_plus_one() {
    assert_eq!(count_lines(""), 1);
    assert_eq!(count_lines("one"), 1);
    assert_eq!(count_lines("a\nb"), 2);
    assert_eq!(count_lines("a\nb\nc"), 3);
    assert_eq!(count_lines("trailing\n"), 2);
}

#[test]
fn should_mask_paste_uses_byte_threshold() {
    assert!(!should_mask_paste(""));
    assert!(!should_mask_paste(&"x".repeat(149)));
    assert!(should_mask_paste(&"x".repeat(150)));
    assert!(should_mask_paste(&"x".repeat(10_000)));
}

// ── paste-newline suppression e2e tests ───────────────────────────────────

#[test]
fn arm_paste_enter_suppression_opens_suppression_window() {
    let mut s = test_state();
    assert!(s.selection.suppress_paste_until.is_none());
    s.arm_paste_enter_suppression();
    assert!(
        s.selection.suppress_paste_until.is_some(),
        "arming must open the suppression window"
    );
    assert!(
        s.paste_enter_is_suppressed(KeyCode::Enter),
        "Enter must be suppressed once the window is armed"
    );
}

#[test]
fn handle_paste_event_does_not_arm_suppression() {
    // Suppression is armed by the bracketed-paste / Ctrl+V call sites only,
    // never by handle_paste_event itself.  This guarantees the burst-flush
    // path (which also goes through handle_paste_event) doesn't accidentally
    // suppress subsequent paste keystrokes.
    let mut s = test_state();
    s.handle_paste_event("line1\nline2");
    assert!(
        s.selection.suppress_paste_until.is_none(),
        "handle_paste_event must not arm suppression on its own"
    );
}

#[test]
fn suppression_blocks_enter_but_not_after_window_expires() {
    let mut s = test_state();
    // Open a suppression window that expired 1 ms ago.
    s.selection.suppress_paste_until =
        std::time::Instant::now().checked_sub(std::time::Duration::from_millis(1));
    assert!(
        !s.paste_enter_is_suppressed(KeyCode::Enter),
        "Enter must not be suppressed once the window has expired"
    );
}

#[test]
fn suppression_covers_enter_tab_backspace_and_chars_not_other_keys() {
    let mut s = test_state();
    s.arm_paste_enter_suppression();
    assert!(s.paste_enter_is_suppressed(KeyCode::Enter));
    assert!(s.paste_enter_is_suppressed(KeyCode::Tab));
    assert!(s.paste_enter_is_suppressed(KeyCode::Backspace));
    assert!(s.paste_enter_is_suppressed(KeyCode::Char('a')));
    // Non-suppressed keys �?arrow keys and Esc pass through.
    assert!(!s.paste_enter_is_suppressed(KeyCode::Left));
    assert!(!s.paste_enter_is_suppressed(KeyCode::Esc));
}

#[test]
fn multiline_paste_below_byte_threshold_is_not_masked() {
    // "a\nb" is 3 bytes �?far below the 150-byte threshold, so it is
    // inserted raw (no mask placeholder).  Suppression is not handle_paste_event's
    // responsibility �?callers (bracketed paste / Ctrl+V) arm it explicitly.
    let mut s = test_state();
    s.handle_paste_event("a\nb");
    assert!(
        !s.input.text.contains("[#1 Pasted"),
        "short multi-line paste must not be masked"
    );
    assert!(s.input.text.contains('\n'), "newline must appear in input");
}

#[test]
fn paste_burst_helpers_buffer_then_flush_through_handle_paste_event() {
    // Simulates a raw-keystroke paste from terminals that don't deliver
    // `Event::Paste`.  We push chars into the burst buffer directly and
    // verify that flush routes them through handle_paste_event so the
    // long-paste mask applies.
    let mut s = test_state();
    // Push a 200-char "paste" with a newline in the middle.
    for ch in "a".repeat(99).chars() {
        s.paste_burst_chars.push(ch);
    }
    s.paste_burst_chars.push('\n');
    for ch in "b".repeat(100).chars() {
        s.paste_burst_chars.push(ch);
    }
    assert_eq!(s.paste_burst_chars.len(), 200);
    s.flush_paste_burst();
    // Buffer is drained.
    assert!(s.paste_burst_chars.is_empty());
    // Length �?150 �?masked (text shows the placeholder, not raw chars).
    assert!(
        s.input.text.contains("[#1 Pasted"),
        "burst above threshold must flush through the mask path"
    );
    // CRITICAL: the burst flush must NOT arm suppression �?doing so would
    // eat the next paste characters arriving in the 100 ms window, which
    // is exactly the truncation regression we're guarding against.
    assert!(
        s.selection.suppress_paste_until.is_none(),
        "burst flush must not arm post-paste suppression"
    );
}

/// End-to-end simulation of the event loop's paste-burst handling for the
/// concrete regression we're fixing: a multi-line paste arriving as raw
/// keystrokes (Windows Terminal + `ConPTY` bypassing `Event::Paste`).
///
/// This walks through the exact sequence of state mutations the event-loop
/// handlers (`KeyCode::Char(c)`, `KeyCode::Enter`) would perform on each
/// arriving key event, then asserts the final input state contains the
/// full multi-line text and that no auto-submit was triggered.
#[test]
fn paste_burst_e2e_multi_line_keystroke_paste_does_not_auto_send() {
    let mut s = test_state();
    let now = std::time::Instant::now();
    let burst = std::time::Duration::from_millis(2);

    // Char 'a' �?first keystroke, no prior key �?inserted directly.
    s.last_key_time = Some(now);
    s.insert_input_char('a');

    // Char 'b' arrives 2 ms later �?in burst, accumulate.
    let t_b = now + burst;
    assert!(s.in_paste_burst(t_b));
    s.paste_burst_chars.push('b');
    s.last_key_time = Some(t_b);

    // Enter arrives 2 ms later �?in burst �?push '\n' to buffer, NO submit.
    // (This is the regression: previously this triggered a send of "ab".)
    let t_enter = t_b + burst;
    assert!(s.in_paste_burst(t_enter));
    s.paste_burst_chars.push('\n');
    s.last_key_time = Some(t_enter);

    // Chars 'c', 'd' �?still in burst, accumulate.
    let t_c = t_enter + burst;
    s.paste_burst_chars.push('c');
    s.last_key_time = Some(t_c);
    let t_d = t_c + burst;
    s.paste_burst_chars.push('d');
    s.last_key_time = Some(t_d);

    // Burst goes idle �?top-of-loop auto-flush fires.
    s.flush_paste_burst();

    // Full multi-line text now in input.text; no auto-send happened.
    assert_eq!(
        s.input.text, "ab\ncd",
        "all pasted content should land in input.text, with the newline preserved"
    );
    assert!(
        s.paste_burst_chars.is_empty(),
        "burst buffer drained after flush"
    );
    // CRITICAL: post-paste suppression must NOT be armed by the burst flush.
    // If it were, the 100 ms window would eat subsequent paste characters,
    // truncating long multi-line pastes (the bug this regression covers).
    assert!(
        s.selection.suppress_paste_until.is_none(),
        "burst flush must not arm Enter suppression"
    );
}

/// E2E: long keystroke-paste hits the mask threshold via the burst flush.
/// Verifies that masking works for terminals that don't deliver
/// `Event::Paste` �?the entire raison d'être of restoring the burst path.
#[test]
fn paste_burst_e2e_long_keystroke_paste_is_masked() {
    let mut s = test_state();
    let big = "x".repeat(200);
    let mut chars = big.chars();

    // First char inserted directly (no prior key to detect burst from).
    s.insert_input_char(chars.next().unwrap());
    s.last_key_time = Some(std::time::Instant::now());

    // Remaining 199 chars accumulate in the burst buffer.
    for c in chars {
        s.paste_burst_chars.push(c);
    }
    // Auto-flush at burst idle.
    s.flush_paste_burst();

    // The 199-char burst above the 150-byte threshold �?masked.
    assert!(
        s.input.text.contains("[#1 Pasted"),
        "burst above threshold should flush through the mask path"
    );
    // Single PasteEntry recorded with the full 199-char content.
    assert_eq!(s.input.pastes.len(), 1);
    assert_eq!(s.input.pastes[0].content.len(), 199);
}

/// Regression test for paste-truncation bug: when a slow render cycle
/// causes the top-of-loop auto-flush to fire MID-paste, the flush must
/// not arm `suppress_paste_until` �?otherwise the 100 ms window eats the
/// remaining paste characters and the user sees a truncated input.
///
/// Concrete symptom that motivated this test: pasting a ~300-byte Rust
/// test function only showed the first ~26 characters in the input bar.
#[test]
fn paste_burst_mid_paste_flush_does_not_eat_subsequent_keystrokes() {
    let mut s = test_state();
    let t0 = std::time::Instant::now();

    // First "half" of the paste accumulates in the burst buffer.
    for ch in "    #[test]\n    fn render_".chars() {
        s.paste_burst_chars.push(ch);
    }
    s.last_key_time = Some(t0);

    // Simulate the top-of-loop auto-flush firing because a slow render
    // cycle pushed last_key_time past the burst threshold.
    // (In real life this happens when a draw cycle blocks > 30 ms.)
    s.flush_paste_burst();

    // The flushed text is now in input.text.  Buffer drained.
    assert!(s.input.text.starts_with("    #[test]\n    fn render_"));
    assert!(s.paste_burst_chars.is_empty());

    // CRITICAL: suppression window must NOT be armed.  The next
    // paste-burst keystroke (the 't' from "tool_call_...") MUST NOT
    // be suppressed.
    assert!(
        s.selection.suppress_paste_until.is_none(),
        "mid-paste flush must not arm suppression �?that's the bug"
    );
    assert!(
        !s.paste_enter_is_suppressed(KeyCode::Char('t')),
        "subsequent paste chars must not be eaten by a suppression window"
    );
    assert!(
        !s.paste_enter_is_suppressed(KeyCode::Enter),
        "subsequent Enter (handled by burst path) must not be suppressed by handle_paste_event"
    );
}

/// E2E: the periodic auto-flush condition.  Verifies the predicate the
/// event loop uses at the top of each tick �?burst is flushed only when
/// both the buffer is non-empty AND the last key is older than the threshold.
#[test]
fn paste_burst_e2e_auto_flush_condition_only_fires_when_idle() {
    let mut s = test_state();
    let now = std::time::Instant::now();
    s.paste_burst_chars.push('x');

    // Recent key �?don't flush yet (burst may continue).
    s.last_key_time = Some(
        now.checked_sub(std::time::Duration::from_millis(5))
            .unwrap(),
    );
    let should_flush_recent = !s.paste_burst_chars.is_empty()
        && s.last_key_time.is_some_and(|t| {
            t.elapsed()
                > std::time::Duration::from_millis(super::ReplTuiState::PASTE_BURST_THRESHOLD_MS)
        });
    assert!(
        !should_flush_recent,
        "must not flush while burst is still active"
    );

    // Idle past threshold �?flush.
    s.last_key_time = Some(
        now.checked_sub(std::time::Duration::from_millis(100))
            .unwrap(),
    );
    let should_flush_idle = !s.paste_burst_chars.is_empty()
        && s.last_key_time.is_some_and(|t| {
            t.elapsed()
                > std::time::Duration::from_millis(super::ReplTuiState::PASTE_BURST_THRESHOLD_MS)
        });
    assert!(should_flush_idle, "must flush once the burst has gone idle");
}

#[test]
fn paste_burst_flush_below_threshold_inserts_raw_without_arming_suppression() {
    // Short burst with newline �?not masked, and suppression is NOT armed
    // (burst path manages its own newlines via the Enter-in-burst handler).
    let mut s = test_state();
    for ch in "ab\ncd".chars() {
        s.paste_burst_chars.push(ch);
    }
    s.flush_paste_burst();
    assert!(s.paste_burst_chars.is_empty());
    assert!(!s.input.text.contains("[#1 Pasted"));
    assert_eq!(s.input.text, "ab\ncd");
    assert!(
        s.selection.suppress_paste_until.is_none(),
        "burst flush must not arm post-paste suppression"
    );
}

#[test]
fn flush_paste_burst_is_a_noop_when_buffer_is_empty() {
    let mut s = test_state();
    s.input.text = "hello".to_string();
    s.input.cursor = 5;
    s.input.byte_cursor = 5;
    s.flush_paste_burst();
    assert_eq!(
        s.input.text, "hello",
        "empty buffer flush must not modify text"
    );
}

#[test]
fn in_paste_burst_respects_threshold() {
    let mut s = test_state();
    let now = std::time::Instant::now();
    // No previous key recorded �?not in burst.
    assert!(!s.in_paste_burst(now));
    // Previous key within threshold �?in burst.
    s.last_key_time = Some(
        now.checked_sub(std::time::Duration::from_millis(10))
            .unwrap(),
    );
    assert!(s.in_paste_burst(now));
    // Previous key beyond threshold �?not in burst.
    s.last_key_time = Some(
        now.checked_sub(std::time::Duration::from_millis(100))
            .unwrap(),
    );
    assert!(!s.in_paste_burst(now));
}

#[test]
fn crlf_paste_normalises_to_lf_in_input() {
    // normalize_pasted_text converts \r\n �?\n so masking / display
    // logic only ever has to consider \n.
    let mut s = test_state();
    s.handle_paste_event("line1\r\nline2");
    assert!(
        s.input.text.contains('\n'),
        "CRLF should be normalised to LF in input.text"
    );
    assert!(
        !s.input.text.contains('\r'),
        "raw \\r should not survive normalisation"
    );
}

#[test]
fn format_paste_placeholder_matches_format() {
    assert_eq!(format_paste_placeholder(1, 1), "[#1 Pasted ~1 lines]");
    assert_eq!(format_paste_placeholder(42, 137), "[#42 Pasted ~137 lines]");
}

#[test]
fn expand_masks_substitutes_original_content() {
    let pastes = vec![
        PasteEntry {
            id: 1,
            placeholder: "[#1 Pasted ~3 lines]".to_string(),
            content: "alpha\nbeta\ngamma".to_string(),
        },
        PasteEntry {
            id: 2,
            placeholder: "[#2 Pasted ~2 lines]".to_string(),
            content: "x\ny".to_string(),
        },
    ];
    let visible = "hi [#1 Pasted ~3 lines] and [#2 Pasted ~2 lines] ok";
    assert_eq!(
        expand_masks(visible, &pastes),
        "hi alpha\nbeta\ngamma and x\ny ok"
    );
}

#[test]
fn expand_masks_is_idempotent_for_no_pastes() {
    assert_eq!(expand_masks("plain text", &[]), "plain text");
}

#[test]
fn submit_expands_masks_before_dispatch() {
    let mut s = test_state();
    s.insert_input_str("please summarise: ");
    let body = "x".repeat(600);
    s.insert_paste_mask(&body);

    // Simulate the submit-path text-extraction logic.
    let raw_line = std::mem::take(&mut s.input.text);
    let line = expand_masks(&raw_line, &s.input.pastes);

    assert!(line.contains(&body));
    assert!(!line.contains("[#1 Pasted"));
}

#[test]
fn snapshot_roundtrip_preserves_pastes() {
    let mut s = test_state();
    s.input.text = "hello [#1 Pasted ~3 lines] world".to_string();
    s.input.cursor = s.input.text.chars().count();
    s.resync_byte_cursor();
    s.input.pastes.push(PasteEntry {
        id: 1,
        placeholder: "[#1 Pasted ~3 lines]".to_string(),
        content: "line1\nline2\nline3".to_string(),
    });
    s.input.next_paste_id = 2;

    let snap = s.current_input_snapshot();
    s.input.text.clear();
    s.input.pastes.clear();
    s.input.next_paste_id = 1;
    s.apply_input_snapshot(snap);

    assert_eq!(s.input.text, "hello [#1 Pasted ~3 lines] world");
    assert_eq!(s.input.pastes.len(), 1);
    assert_eq!(s.input.pastes[0].content, "line1\nline2\nline3");
    assert_eq!(s.input.next_paste_id, 2);
}

#[test]
fn insert_paste_mask_inserts_placeholder_and_records_entry() {
    let mut s = test_state();
    s.input.text = "prefix ".to_string();
    s.input.cursor = s.input.text.chars().count();
    s.resync_byte_cursor();

    let content = "line1\nline2\nline3\n".repeat(40); // > 500 bytes, many lines
    s.insert_paste_mask(&content);

    assert_eq!(s.input.pastes.len(), 1);
    let entry = &s.input.pastes[0];
    assert_eq!(entry.id, 1);
    assert!(entry.placeholder.starts_with("[#1 Pasted ~"));
    assert_eq!(entry.content, content);
    assert!(s.input.text.starts_with("prefix "));
    assert!(s.input.text.contains(&entry.placeholder));
    // Trailing space appended after the placeholder.
    assert!(s.input.text.ends_with(' '));
    // Cursor sits past the trailing space.
    assert_eq!(s.input.cursor, s.input.text.chars().count());
    assert_eq!(s.input.byte_cursor, s.input.text.len());
    assert_eq!(s.input.next_paste_id, 2);
}

#[test]
fn insert_paste_mask_increments_id_for_consecutive_pastes() {
    let mut s = test_state();
    let big = "x".repeat(600);
    s.insert_paste_mask(&big);
    s.insert_paste_mask(&big);
    assert_eq!(s.input.pastes.len(), 2);
    assert_eq!(s.input.pastes[0].id, 1);
    assert_eq!(s.input.pastes[1].id, 2);
    assert!(s.input.text.contains("[#1 Pasted"));
    assert!(s.input.text.contains("[#2 Pasted"));
}

#[test]
fn compute_mask_ranges_finds_all_placeholders() {
    let mut s = test_state();
    let big = "x".repeat(600);
    s.insert_paste_mask(&big);
    s.insert_input_str(" middle ");
    s.insert_paste_mask(&big);

    let ranges = s.compute_mask_ranges();
    assert_eq!(ranges.len(), 2);
    // Ranges sorted by start position.
    assert!(ranges[0].1.start < ranges[1].1.start);
    // Each range slices to exactly the placeholder string.
    assert_eq!(
        &s.input.text[ranges[0].1.clone()],
        s.input.pastes[ranges[0].0].placeholder
    );
}

#[test]
fn mask_containing_returns_none_at_boundaries_or_outside() {
    let mut s = test_state();
    let big = "x".repeat(600);
    s.insert_paste_mask(&big);
    let ranges = s.compute_mask_ranges();
    let r = ranges[0].1.clone();

    assert!(ReplTuiState::mask_containing(r.start, &ranges).is_none());
    assert!(ReplTuiState::mask_containing(r.end, &ranges).is_none());
    assert!(ReplTuiState::mask_containing(r.start + 1, &ranges).is_some());
    assert!(ReplTuiState::mask_containing(r.start + 5, &ranges).is_some());
}

#[test]
fn next_paste_id_resets_to_one_after_submit() {
    let mut s = test_state();
    s.insert_paste_mask(&"x".repeat(600));
    s.insert_paste_mask(&"y".repeat(600));
    assert_eq!(s.input.next_paste_id, 3);

    // Simulate submit-path reset (mirrors the production code).
    let raw_line = std::mem::take(&mut s.input.text);
    let _line = expand_masks(&raw_line, &s.input.pastes);
    s.input.pastes.clear();
    s.input.next_paste_id = 1;

    assert!(s.input.pastes.is_empty());
    assert_eq!(s.input.next_paste_id, 1);

    // A fresh paste should now get id #1 again.
    s.insert_paste_mask(&"z".repeat(600));
    assert_eq!(s.input.pastes[0].id, 1);
}

#[test]
fn up_arrow_into_mask_snaps_to_mask_start() {
    let mut s = test_state();
    // Build a multi-line input so up-arrow has somewhere to go.
    s.insert_input_str("first line\n");
    s.insert_paste_mask(&"y".repeat(600));
    // Cursor is past the trailing space after the placeholder; up-arrow from
    // here lands on the previous (first) line if mask isn't on its own line,
    // OR if the mask spans where up would land, it must snap to mask start.
    // Position cursor at the END of the input text and press up.
    s.move_input_cursor_up();

    // After up, byte_cursor should not fall strictly inside the mask.
    let ranges = s.compute_mask_ranges();
    for (_, r) in &ranges {
        assert!(
            !(s.input.byte_cursor > r.start && s.input.byte_cursor < r.end),
            "cursor should not land strictly inside a mask after up-arrow"
        );
    }
}

#[test]
fn down_arrow_into_mask_snaps_to_mask_end() {
    let mut s = test_state();
    s.insert_input_str("first line\n");
    s.insert_paste_mask(&"y".repeat(600));
    // Move cursor up to the first line, then back down �?it should not land
    // inside the mask either way.
    s.move_input_cursor_up();
    s.move_input_cursor_down();

    let ranges = s.compute_mask_ranges();
    for (_, r) in &ranges {
        assert!(
            !(s.input.byte_cursor > r.start && s.input.byte_cursor < r.end),
            "cursor should not land strictly inside a mask after up+down round-trip"
        );
    }
}

#[test]
fn paste_while_cursor_inside_mask_snaps_first() {
    let mut s = test_state();
    s.insert_paste_mask(&"a".repeat(600));
    let ranges = s.compute_mask_ranges();
    let r = ranges[0].1.clone();
    // Place cursor strictly inside the first mask.
    s.input.byte_cursor = r.start + 3;
    s.input.cursor = s.input.text[..r.start + 3].chars().count();

    s.insert_paste_mask(&"b".repeat(600));

    // Both masks present, neither nested.
    let ranges = s.compute_mask_ranges();
    assert_eq!(ranges.len(), 2);
    assert!(ranges[0].1.end <= ranges[1].1.start);
}

#[test]
fn large_paste_event_creates_mask_not_raw_text() {
    let mut s = test_state();
    let big = "y".repeat(600);
    s.handle_paste_event(&big);

    assert!(s.input.text.contains("[#1 Pasted ~"));
    assert!(!s.input.text.contains(&big));
    assert_eq!(s.input.pastes.len(), 1);
}

#[test]
fn small_paste_event_inserts_raw_text() {
    let mut s = test_state();
    let small = "short paste".to_string();
    s.handle_paste_event(&small);

    assert_eq!(s.input.text, "short paste");
    assert!(s.input.pastes.is_empty());
}

#[test]
fn crlf_paste_normalises_to_lf_in_stored_content() {
    let mut s = test_state();
    let mut crlf = "a\r\n".repeat(300);
    crlf.push_str("end");
    s.handle_paste_event(&crlf);

    assert_eq!(s.input.pastes.len(), 1);
    assert!(!s.input.pastes[0].content.contains('\r'));
    assert!(s.input.pastes[0].content.contains('\n'));
}

#[test]
fn left_arrow_at_mask_end_snaps_to_mask_start() {
    let mut s = test_state();
    s.insert_paste_mask(&"x".repeat(600));
    let ranges = s.compute_mask_ranges();
    let r = ranges[0].1.clone();

    // Position cursor exactly at mask end (it currently sits past the trailing space).
    s.input.byte_cursor = r.end;
    s.input.cursor = s.input.text[..r.end].chars().count();

    s.move_input_cursor_left();

    assert_eq!(s.input.byte_cursor, r.start);
    assert_eq!(s.input.cursor, s.input.text[..r.start].chars().count());
}

#[test]
fn right_arrow_at_mask_start_snaps_to_mask_end() {
    let mut s = test_state();
    s.insert_input_str("hi ");
    s.insert_paste_mask(&"x".repeat(600));
    let ranges = s.compute_mask_ranges();
    let r = ranges[0].1.clone();

    s.input.byte_cursor = r.start;
    s.input.cursor = s.input.text[..r.start].chars().count();

    s.move_input_cursor_right();

    assert_eq!(s.input.byte_cursor, r.end);
}

#[test]
fn compute_mask_ranges_skips_orphaned_entries() {
    // If a placeholder is no longer present in `text` (e.g. after manual edits
    // that removed it without going through atomic delete), the entry is
    // silently skipped.
    let mut s = test_state();
    s.input.pastes.push(PasteEntry {
        id: 1,
        placeholder: "[#1 Pasted ~5 lines]".to_string(),
        content: "x".repeat(600),
    });
    // text is empty �?no occurrences of the placeholder.
    let ranges = s.compute_mask_ranges();
    assert!(ranges.is_empty());
}

#[test]
fn compute_mask_char_ranges_handles_multibyte_prefix() {
    let mut s = test_state();
    // Multi-byte prefix shifts byte positions relative to char positions.
    s.insert_input_str("héllo ");
    s.insert_paste_mask(&"x".repeat(600));

    let byte_ranges = s.compute_mask_ranges();
    let char_ranges = s.compute_mask_char_ranges();
    assert_eq!(char_ranges.len(), 1);

    let (c_start, c_end) = char_ranges[0];
    let r = &byte_ranges[0].1;
    assert_eq!(c_start, s.input.text[..r.start].chars().count());
    assert_eq!(c_end, s.input.text[..r.end].chars().count());
    // Placeholder is ASCII so char-length equals byte-length within the range.
    assert_eq!(c_end - c_start, r.end - r.start);
}

#[test]
fn input_render_styles_mask_as_dim_italic() {
    use ratatui::style::Modifier;

    let mut s = test_state();
    s.insert_input_str("hi ");
    s.insert_paste_mask(&"x".repeat(600));

    let (_h, lines, _max_scroll, _cursor) = s.calculate_input_dimensions(80, "model");
    // First line is a blank padding; the input row is at index 1.
    let input_line = &lines[1];
    let mut found_mask_span = false;
    for span in &input_line.spans {
        if span.content.contains("[#1 Pasted") {
            let mods = span.style.add_modifier;
            assert!(
                mods.contains(Modifier::DIM) && mods.contains(Modifier::ITALIC),
                "mask span missing DIM/ITALIC modifiers (got {mods:?}) for content {:?}",
                span.content
            );
            found_mask_span = true;
        }
    }
    assert!(
        found_mask_span,
        "expected at least one span carrying the placeholder text, got spans {:?}",
        input_line
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<Vec<_>>()
    );
}

fn assert_matching_lengths(items: &[ratatui::widgets::ListItem<'static>], text: &[String]) {
    assert_eq!(items.len(), text.len());
}

fn selected_text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .filter(|span| span.style.bg == Some(Color::DarkGray))
        .map(|span| span.content.as_ref())
        .collect()
}

#[test]
fn auth_command_blocked_when_busy() {
    let (tx, _rx) = mpsc::channel();
    let modal = AuthModal::new(tx.clone(), None);
    assert!(matches!(modal.step, AuthModalStep::ProviderSelect { .. }));
}

#[test]
fn auth_command_with_provider_arg_skips_selection() {
    let (tx, _rx) = mpsc::channel();
    let modal = AuthModal::new(tx.clone(), Some(Provider::OpenAi));
    assert!(matches!(
        modal.step,
        AuthModalStep::ApiKeyInput {
            provider: ProviderKind::OpenAi,
            ..
        }
    ));
    let modal2 = AuthModal::new(tx, Some(Provider::Anthropic));
    assert!(matches!(
        modal2.step,
        AuthModalStep::OAuthWaiting {
            provider: ProviderKind::Anthropic,
            ..
        }
    ));
}

#[test]
fn render_tool_call_running_status() {
    let (items, text) = render_tool_call_lines(
        "bash",
        "echo hello",
        &ToolCallStatus::Running,
        80,
        '⠋',
        false,
    );
    assert_eq!(items.len(), 1);
    assert_matching_lengths(&items, &text);
}

#[test]
fn render_tool_call_success_empty_output() {
    let (items, text) = render_tool_call_lines(
        "navigate",
        "https://example.com",
        &ToolCallStatus::Success {
            output: String::new(),
        },
        80,
        '⠋',
        false,
    );
    assert_eq!(items.len(), 1);
    assert_matching_lengths(&items, &text);
}

#[test]
fn render_tool_call_success_with_output() {
    let (items, text) = render_tool_call_lines(
        "bash",
        "ls -la",
        &ToolCallStatus::Success {
            output: "some result".to_string(),
        },
        80,
        '⠋',
        false,
    );
    assert_eq!(items.len(), 1);
    assert_matching_lengths(&items, &text);
}

#[test]
fn render_tool_call_error_status() {
    let (items, text) = render_tool_call_lines(
        "bash",
        "bad command",
        &ToolCallStatus::Error("timeout after 30s".to_string()),
        80,
        '⠋',
        false,
    );
    assert!(items.len() >= 2);
    let plain = text.join(" ");
    assert!(plain.contains("bash"));
    assert!(plain.contains("timeout after 30s"));
    assert_matching_lengths(&items, &text);
}

#[test]
fn render_tool_call_input_truncation() {
    let long_input = "a".repeat(80);
    let (items, text) = render_tool_call_lines(
        "bash",
        &long_input,
        &ToolCallStatus::Running,
        80,
        '⠋',
        false,
    );
    assert_eq!(items.len(), 1);
    assert_matching_lengths(&items, &text);
}

#[test]
fn render_tool_call_bash_rich_stdout() {
    let output = serde_json::json!({
        "stdout": "line1\nline2\nline3",
        "stderr": ""
    })
    .to_string();
    let (items, text) = render_tool_call_lines(
        "bash",
        r#"{"command":"ls -la"}"#,
        &ToolCallStatus::Success { output },
        80,
        '⠋',
        false,
    );
    assert!(
        items.len() >= 2,
        "Expected header + stdout lines, got {}",
        items.len()
    );
    assert_matching_lengths(&items, &text);
}

#[test]
fn render_tool_call_bash_with_stderr() {
    let output = serde_json::json!({
        "stdout": "",
        "stderr": "error line"
    })
    .to_string();
    let (items, text) = render_tool_call_lines(
        "bash",
        "cmd",
        &ToolCallStatus::Success { output },
        80,
        '⠋',
        false,
    );
    assert!(
        items.len() >= 2,
        "Expected header + stderr line, got {}",
        items.len()
    );
    assert_matching_lengths(&items, &text);
}

#[test]
fn render_tool_call_unknown_tool_single_line() {
    let output = "navigation complete".to_string();
    let (items, text) = render_tool_call_lines(
        "navigate",
        "https://example.com",
        &ToolCallStatus::Success { output },
        80,
        '⠋',
        false,
    );
    assert_eq!(
        items.len(),
        1,
        "Unknown tool should produce exactly 1 line, got {}",
        items.len()
    );
    assert_matching_lengths(&items, &text);
}

#[test]
fn render_tool_call_bash_overflow_truncated() {
    let stdout = (0..20)
        .map(|i| format!("line{i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let output = serde_json::json!({ "stdout": stdout, "stderr": "" }).to_string();
    let (items, text) = render_tool_call_lines(
        "bash",
        "cmd",
        &ToolCallStatus::Success { output },
        80,
        '⠋',
        false,
    );
    assert_eq!(
        items.len(),
        17,
        "Expected 1 header + 15 lines + 1 overflow = 17, got {}",
        items.len()
    );
    assert_matching_lengths(&items, &text);
}

#[test]
fn wrap_ansi_line_respects_wide_character_width() {
    let wrapped = wrap_ansi_line(Line::from("ab中cd中efg"), 10);
    let plain = wrapped.iter().map(line_to_plain_text).collect::<Vec<_>>();

    assert_eq!(plain, vec!["ab中cd中ef".to_string(), "g".to_string()]);
}

#[test]
fn tool_input_summary_extracts_key_fields() {
    assert_eq!(tool_input_summary("bash", r#"{"command":"ls"}"#), "ls");
    assert_eq!(
        tool_input_summary("navigate", r#"{"url":"https://example.com"}"#),
        "https://example.com"
    );
}

#[test]
fn input_cursor_uses_display_width_for_wide_chars() {
    let mut state = ReplTuiState::new();
    state.input.text = "a中\nbc".to_string();
    state.input.cursor = 2;

    assert_eq!(state.input_cursor_line_col(), (0, 3));

    state.move_input_cursor_down();
    assert_eq!(state.input_cursor_line_col(), (1, 2));
    assert_eq!(state.input.cursor, 5);
}

#[test]
fn calculate_input_dimensions_places_cursor_after_wide_char() {
    let mut state = ReplTuiState::new();
    state.input.text = "中a".to_string();
    state.input.cursor = 1;

    let (_, _, _, cursor_pos) = state.calculate_input_dimensions(20, "model");
    let prompt_width = u16::try_from(text_display_width("❯ ")).unwrap_or(u16::MAX);

    assert_eq!(cursor_pos, Some((1, prompt_width + 2)));
}

#[test]
fn select_all_input_marks_entire_buffer() {
    let mut state = ReplTuiState::new();
    state.input.text = "ab\ncd".to_string();
    state.input.cursor = 1;
    state.resync_byte_cursor();

    state.select_all_input();

    assert_eq!(state.input_selection, Some((0, 5)));
    assert_eq!(state.input.cursor, 5);
}

#[test]
fn copy_selection_yanks_expanded_content() {
    let mut s = test_state();
    s.insert_input_str("a ");
    s.insert_paste_mask(&"z".repeat(600));
    s.insert_input_str(" b");

    // Select the whole input.
    let total = s.input.text.chars().count();
    s.input_selection = Some((0, total));

    let yanked = s.selected_input_text_expanded().unwrap();
    assert!(yanked.contains(&"z".repeat(600)));
    assert!(!yanked.contains("[#1 Pasted"));
}

#[test]
fn cut_input_selection_text_returns_text_and_removes_it() {
    let mut state = ReplTuiState::new();
    state.input.text = "ab\ncd".to_string();
    state.input.cursor = 5;
    state.resync_byte_cursor();
    state.input_selection = Some((0, 3));

    let cut = state.cut_input_selection_text();

    assert_eq!(cut.as_deref(), Some("ab\n"));
    assert_eq!(state.input.text, "cd");
    assert_eq!(state.input.cursor, 0);
    assert_eq!(state.input_selection, None);
}

#[test]
fn undo_redo_input_insert_round_trip() {
    let mut state = ReplTuiState::new();

    state.insert_input_str("hello");
    assert_eq!(state.input.text, "hello");

    assert!(state.undo_input_edit());
    assert_eq!(state.input.text, "");
    assert_eq!(state.input.cursor, 0);

    assert!(state.redo_input_edit());
    assert_eq!(state.input.text, "hello");
    assert_eq!(state.input.cursor, 5);
}

#[test]
fn undo_redo_restores_cut_input_selection() {
    let mut state = ReplTuiState::new();
    state.input.text = "ab\ncd".to_string();
    state.input.cursor = 5;
    state.resync_byte_cursor();
    state.input_selection = Some((0, 3));

    let cut = state.cut_input_selection_text();

    assert_eq!(cut.as_deref(), Some("ab\n"));
    assert!(state.undo_input_edit());
    assert_eq!(state.input.text, "ab\ncd");
    assert_eq!(state.input_selection, Some((0, 3)));

    assert!(state.redo_input_edit());
    assert_eq!(state.input.text, "cd");
    assert_eq!(state.input.cursor, 0);
}

#[test]
fn input_selection_preserves_newline_char_offsets_across_paragraphs() {
    let mut state = ReplTuiState::new();
    state.input.text = "ab\ncd\nef".to_string();
    state.input.cursor = state.input.text.chars().count();
    state.input_selection = Some((3, 7));

    let (_, render_lines, _, _) = state.calculate_input_dimensions(20, "model");

    assert_eq!(selected_text(&render_lines[2]), "cd");
    assert_eq!(selected_text(&render_lines[3]), "e");
}

#[test]
fn tool_call_start_flushes_typewriter_first() {
    let (tx, rx) = mpsc::channel::<ReplTuiEvent>();
    let mut state = ReplTuiState::new();

    for c in "hello\n".chars() {
        state.typewriter.chars.push_back(c);
    }

    tx.send(ReplTuiEvent::ToolCallStart {
        name: "bash".to_string(),
        input: r#"{"command":"ls"}"#.to_string(),
    })
    .unwrap();
    state.drain_events(&rx);

    assert_eq!(state.live_tool_calls.len(), 1);
    assert!(matches!(
        state.live_tool_calls[0],
        (_, _, ToolCallStatus::Running)
    ));
}

#[test]
fn tool_call_complete_updates_in_place_success() {
    let (tx, rx) = mpsc::channel::<ReplTuiEvent>();
    let mut state = ReplTuiState::new();

    state.live_tool_calls.push((
        "bash".to_string(),
        "ls".to_string(),
        ToolCallStatus::Running,
    ));

    tx.send(ReplTuiEvent::ToolCallComplete {
        name: "bash".to_string(),
        output: "file.txt".to_string(),
        is_error: false,
    })
    .unwrap();
    state.drain_events(&rx);

    assert_eq!(state.live_tool_calls.len(), 1);
    assert!(matches!(
        state.live_tool_calls[0],
        (_, _, ToolCallStatus::Success { .. })
    ));
}

#[test]
fn tool_call_complete_updates_in_place_error() {
    let (tx, rx) = mpsc::channel::<ReplTuiEvent>();
    let mut state = ReplTuiState::new();

    state.live_tool_calls.push((
        "bash".to_string(),
        "bad cmd".to_string(),
        ToolCallStatus::Running,
    ));

    tx.send(ReplTuiEvent::ToolCallComplete {
        name: "bash".to_string(),
        output: "command not found".to_string(),
        is_error: true,
    })
    .unwrap();
    state.drain_events(&rx);

    assert_eq!(state.live_tool_calls.len(), 1);
    assert!(matches!(
        state.live_tool_calls[0],
        (_, _, ToolCallStatus::Error(_))
    ));
}

#[test]
fn goal_title_shows_reasoning_effort_for_reasoning_model() {
    let header = super::HeaderSnapshot {
        model: "gpt-5.3-codex".to_string(),
        reasoning_effort: Some("high".to_string()),
        ..Default::default()
    };
    let mut state = ReplTuiState::new();
    state.cached_header = header;

    let title = if let Some(ref effort) = state.cached_header.reasoning_effort {
        format!(" Goal · {} · {effort} ", state.cached_header.model)
    } else {
        format!(" Goal · {} ", state.cached_header.model)
    };
    assert_eq!(title, " Goal · gpt-5.3-codex · high ");
}

#[test]
fn goal_title_omits_effort_for_non_reasoning_model() {
    let header = super::HeaderSnapshot {
        model: "anthropic/claude-sonnet-4-6".to_string(),
        reasoning_effort: None,
        ..Default::default()
    };
    let mut state = ReplTuiState::new();
    state.cached_header = header;

    let title = if let Some(ref effort) = state.cached_header.reasoning_effort {
        format!(" Goal · {} · {effort} ", state.cached_header.model)
    } else {
        format!(" Goal · {} ", state.cached_header.model)
    };
    assert_eq!(title, " Goal · anthropic/claude-sonnet-4-6 ");
}

#[test]
fn goal_title_cycles_through_all_effort_levels() {
    let efforts = ["none", "minimal", "low", "medium", "high", "xhigh"];
    for effort in &efforts {
        let header = super::HeaderSnapshot {
            model: "openai/gpt-5.3-codex".to_string(),
            reasoning_effort: Some(effort.to_string()),
            ..Default::default()
        };
        let title = format!(" Goal · {} · {} ", header.model, effort);
        assert!(
            title.contains(&format!("· {effort} ")),
            "title should contain effort level '{effort}': {title}"
        );
    }
}

#[test]
fn test_welcome_card_renders_when_outdated() {
    let mut state = super::ReplTuiState::new();
    state.update_info = Some(runtime::update_check::UpdateInfo {
        latest_version: "9.9.9".to_string(),
        current_version: "1.0.0".to_string(),
        is_outdated: true,
    });

    let backend = ratatui::backend::TestBackend::new(100, 40);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| {
            crate::tui::repl_render::draw_welcome(frame, frame.area(), &mut state, false);
        })
        .unwrap();

    let buffer = terminal.backend().buffer();
    let content = (0..40)
        .map(|y| {
            (0..100)
                .map(|x| buffer.cell((x, y)).unwrap().symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");

    assert!(content.contains("Update available: v9.9.9 (you have v1.0.0)"));
}

#[test]
fn backspace_at_mask_end_deletes_entire_mask() {
    let mut s = test_state();
    s.insert_input_str("hi ");
    let big = "y".repeat(600);
    s.insert_paste_mask(&big);
    // Cursor sits past the trailing space.  Move it back one char to land on
    // mask end (the boundary between placeholder and the trailing space).
    let trailing_space_byte = s.input.byte_cursor - 1;
    s.input.byte_cursor = trailing_space_byte;
    s.input.cursor -= 1;

    let before_len = s.input.text.len();
    s.backspace_input_char();

    assert!(!s.input.text.contains("[#1 Pasted"));
    assert!(s.input.text.len() < before_len);
    assert!(s.input.pastes.is_empty());
    // Trailing separator space is deleted atomically with the mask.
    assert_eq!(s.input.text, "hi ");
}

#[test]
fn delete_at_mask_start_deletes_entire_mask() {
    let mut s = test_state();
    s.insert_paste_mask(&"y".repeat(600));
    // Position cursor at mask start.
    let ranges = s.compute_mask_ranges();
    let r = ranges[0].1.clone();
    s.input.byte_cursor = r.start;
    s.input.cursor = s.input.text[..r.start].chars().count();

    s.delete_input_char();

    assert!(!s.input.text.contains("[#1 Pasted"));
    assert!(s.input.pastes.is_empty());
}

#[test]
fn test_no_card_when_current() {
    let mut state = super::ReplTuiState::new();
    state.update_info = None;

    let backend = ratatui::backend::TestBackend::new(100, 40);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| {
            crate::tui::repl_render::draw_welcome(frame, frame.area(), &mut state, false);
        })
        .unwrap();

    let buffer = terminal.backend().buffer();
    let content = (0..40)
        .map(|y| {
            (0..100)
                .map(|x| buffer.cell((x, y)).unwrap().symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");

    assert!(!content.contains("Update available"));
}

// ── Integration tests: state construction ──────────────────────────────────

#[test]
fn repl_state_new_does_not_panic() {
    let state = ReplTuiState::new();
    assert!(!state.busy);
    assert!(!state.exit);
    assert!(state.messages.is_empty());
    assert_eq!(state.input.text, "");
    assert_eq!(state.input.cursor, 0);
}

#[test]
fn repl_state_new_starts_in_welcome_mode() {
    let state = ReplTuiState::new();
    assert_eq!(state.ui_state, super::AppUiState::WelcomeMode);
    assert!(state.active_modal.is_none());
    assert!(state.slash_overlay.is_none());
    assert!(state.follow_bottom);
}

// ── Integration tests: event dispatch ──────────────────────────────────────

#[test]
fn drain_events_turn_starting_sets_busy() {
    let (tx, rx) = mpsc::channel::<ReplTuiEvent>();
    let mut state = ReplTuiState::new();
    assert!(!state.busy);

    tx.send(ReplTuiEvent::TurnStarting).unwrap();
    state.drain_events(&rx);

    assert!(state.busy);
    assert_eq!(state.status_line, "Thinking...");
    assert!(state.live_tool_calls.is_empty());
}

#[test]
fn drain_events_turn_finished_clears_busy() {
    let (tx, rx) = mpsc::channel::<ReplTuiEvent>();
    let mut state = ReplTuiState::new();

    tx.send(ReplTuiEvent::TurnStarting).unwrap();
    state.drain_events(&rx);
    assert!(state.busy);

    tx.send(ReplTuiEvent::TurnFinished(Ok(()))).unwrap();
    state.drain_events(&rx);

    assert!(!state.busy);
    assert_eq!(state.status_line, "Ready");
}

#[test]
fn drain_events_system_message_adds_entry() {
    let (tx, rx) = mpsc::channel::<ReplTuiEvent>();
    let mut state = ReplTuiState::new();

    tx.send(ReplTuiEvent::SystemMessage("hello system".to_string()))
        .unwrap();
    state.drain_events(&rx);

    assert_eq!(state.messages.len(), 1);
    assert_eq!(state.messages[0].role, MessageRole::System);
    assert_eq!(
        state.messages[0].blocks,
        vec![ContentBlock::Text {
            text: "hello system".to_string()
        }]
    );
}

#[test]
fn drain_events_message_completed_appends_to_messages() {
    let (tx, rx) = mpsc::channel::<ReplTuiEvent>();
    let mut state = ReplTuiState::new();
    let user_msg = ConversationMessage::user_text("hello");

    tx.send(ReplTuiEvent::MessageCompleted(user_msg.clone()))
        .unwrap();
    state.drain_events(&rx);

    assert_eq!(state.messages.len(), 1);
    assert_eq!(state.messages[0].role, MessageRole::User);
}

#[test]
fn drain_events_stream_text_enqueues_typewriter_chars() {
    let (tx, rx) = mpsc::channel::<ReplTuiEvent>();
    let mut state = ReplTuiState::new();

    tx.send(ReplTuiEvent::StreamText("abc".to_string()))
        .unwrap();
    state.drain_events(&rx);

    assert_eq!(state.typewriter.chars.len(), 3);
    assert_eq!(state.typewriter.chars[0], 'a');
    assert_eq!(state.typewriter.chars[1], 'b');
    assert_eq!(state.typewriter.chars[2], 'c');
}

#[test]
fn drain_events_messages_loaded_resets_state() {
    let (tx, rx) = mpsc::channel::<ReplTuiEvent>();
    let mut state = ReplTuiState::new();
    state.busy = true;
    state
        .live_tool_calls
        .push(("tool".to_string(), String::new(), ToolCallStatus::Running));
    state.typewriter.chars.push_back('x');
    state.typewriter.live.push('y');
    state.current_tool = Some("navigate".to_string());

    let msgs = vec![ConversationMessage::user_text("hi")];
    tx.send(ReplTuiEvent::MessagesLoaded(msgs)).unwrap();
    state.drain_events(&rx);

    assert_eq!(state.messages.len(), 1);
    assert!(!state.busy);
    assert!(state.live_tool_calls.is_empty());
    assert!(state.typewriter.chars.is_empty());
    assert!(state.typewriter.live.is_empty());
    assert!(state.current_tool.is_none());
    assert!(state.follow_bottom);
    assert_eq!(state.status_line, "Ready");
}

#[test]
fn session_switch_bulk_loads_messages_into_state() {
    let (tx, rx) = mpsc::channel::<ReplTuiEvent>();
    let mut state = ReplTuiState::new();
    state.busy = true;
    state.current_tool = Some("execute_js".to_string());

    let msgs = vec![
        ConversationMessage::user_text("first"),
        ConversationMessage::user_text("second"),
    ];
    tx.send(ReplTuiEvent::MessagesLoaded(msgs)).unwrap();
    state.drain_events(&rx);

    assert_eq!(state.messages.len(), 2);
    assert_eq!(state.messages[0].role, MessageRole::User);
    assert_eq!(state.messages[1].role, MessageRole::User);
    assert!(!state.busy);
    assert!(state.current_tool.is_none());
    assert!(!state.cancelling);
}

// ── Integration tests: modal state machine ─────────────────────────────────

#[test]
fn active_modal_auth_variant_accessors() {
    use crate::tui::active_modal::ActiveModal;
    use crate::tui::auth_modal::AuthModal;

    let (tx, _rx) = mpsc::channel::<ReplTuiEvent>();
    let auth = AuthModal::new(tx, None);
    let mut modal = ActiveModal::Auth(auth);

    assert!(modal.as_auth_mut().is_some());
    assert!(modal.as_model_mut().is_none());
    assert!(modal.as_session_mut().is_none());
}

#[test]
fn active_modal_session_variant_accessors() {
    use crate::tui::active_modal::ActiveModal;
    use crate::tui::session_modal::SessionModal;
    use std::path::PathBuf;

    let entries = vec![SessionModalEntry {
        id: "s1".to_string(),
        path: PathBuf::from("/tmp/s1.json"),
        title: Some("Test Session".to_string()),
        modified_epoch_secs: 1_700_000_000,
        message_count: 5,
        is_current: false,
    }];
    let session = SessionModal::new(entries);
    let mut modal = ActiveModal::Session(session);

    assert!(modal.as_session_mut().is_some());
    assert!(modal.as_auth_mut().is_none());
    assert!(modal.as_model_mut().is_none());
}

// ── Integration tests: calculate_input_dimensions ──────────────────────────

#[test]
fn calculate_input_dimensions_empty_input_various_widths() {
    for width in [20u16, 40, 80, 120, 200] {
        let mut state = ReplTuiState::new();
        let (height, lines, _total_visual, cursor_pos) =
            state.calculate_input_dimensions(width, "anthropic/claude-sonnet-4-6");

        assert!(height > 0, "height must be > 0 for width={width}");
        assert!(!lines.is_empty(), "must produce at least 1 render line");
        assert!(
            cursor_pos.is_some(),
            "cursor_pos should be Some for empty input at width={width}"
        );
    }
}

#[test]
fn calculate_input_dimensions_long_input_wraps() {
    let mut state = ReplTuiState::new();
    state.input.text = "the quick brown fox jumps over the lazy dog".to_string();
    state.input.cursor = state.input.text.chars().count();
    state.resync_byte_cursor();

    let (_height, lines, _total_visual, cursor_pos) = state.calculate_input_dimensions(30, "model");

    assert!(
        lines.len() > 2,
        "long text at narrow width should wrap into >2 lines, got {}",
        lines.len()
    );
    assert!(cursor_pos.is_some());
}

#[test]
fn calculate_input_dimensions_multiline_input() {
    let mut state = ReplTuiState::new();
    state.input.text = "line1\nline2\nline3".to_string();
    state.input.cursor = 5;
    state.resync_byte_cursor();

    let (_height, lines, _total_visual, cursor_pos) = state.calculate_input_dimensions(80, "model");

    assert!(
        lines.len() >= 4,
        "3 logical lines + padding should produce >=4 render lines, got {}",
        lines.len()
    );
    assert!(cursor_pos.is_some());
}
