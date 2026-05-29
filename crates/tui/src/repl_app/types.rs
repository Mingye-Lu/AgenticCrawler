use super::*;

#[derive(Clone, Debug)]
pub enum ToolCallStatus {
    Running,
    Interrupted,
    Success { output: String },
    Error(String),
}

#[derive(Debug, Clone)]
pub(crate) struct SlashOverlayItem {
    pub(crate) command: String,
    pub(crate) summary: &'static str,
}

#[derive(Debug, Clone)]
pub(crate) struct SlashOverlay {
    pub(crate) items: Vec<SlashOverlayItem>,
    pub(crate) selected: usize,
    pub(crate) scroll_offset: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct HeaderSnapshot {
    pub(crate) model: String,
    pub(crate) session_id: String,
    pub(crate) cost_text: String,
    pub(crate) context_text: String,
    pub(crate) reasoning_effort: Option<String>,
}

#[derive(Clone, Debug)]
pub(super) enum ModelCatalogState {
    Loading,
    Ready(Vec<api::provider::ModelInfo>),
    Failed,
}

impl Default for HeaderSnapshot {
    fn default() -> Self {
        Self {
            model: "--".to_string(),
            session_id: "--".to_string(),
            cost_text: "--".to_string(),
            context_text: "--".to_string(),
            reasoning_effort: None,
        }
    }
}

pub(super) struct MouseCaptureGuard;

impl Drop for MouseCaptureGuard {
    fn drop(&mut self) {
        let _ = execute!(io::stdout(), event::DisableMouseCapture);
        let _ = execute!(io::stdout(), DisableBracketedPaste);
    }
}

pub(super) enum WorkerMsg {
    RunTurn(String),
    Shutdown,
}

pub(super) struct InputEditorState {
    pub(super) text: String,
    pub(super) cursor: usize,
    /// Byte-level position matching `cursor` —avoids O(n) `char_indices().nth()`
    /// scans in hot paths (paste, render).  Invalidated and lazily re-synced
    /// when the cursor is set directly (clamp / `set_line_col`).
    pub(super) byte_cursor: usize,
    pub(super) preferred_col: Option<usize>,
    /// Active paste masks, in insertion order.  Empty when no pastes are masked.
    pub(super) pastes: Vec<PasteEntry>,
    /// Monotonically increases as masks are inserted within one prompt; reset to
    /// 1 on submit, clear, or new-prompt boundary.
    pub(super) next_paste_id: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct InputUndoSnapshot {
    pub(super) text: String,
    pub(super) cursor: usize,
    pub(super) preferred_col: Option<usize>,
    pub(super) selection: Option<(usize, usize)>,
    pub(super) pastes: Vec<PasteEntry>,
    pub(super) next_paste_id: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct PasteEntry {
    /// 1-based, per-prompt index. Resets to 1 on submit / clear.
    pub(super) id: u32,
    /// Visible placeholder, e.g. "[#1 Pasted ~42 lines]".
    /// Uniqueness is guaranteed by `id`, so `text.find(&placeholder)` is safe.
    pub(super) placeholder: String,
    /// Original pasted content (after newline normalisation).
    pub(super) content: String,
}

#[derive(Default)]
pub(crate) struct SelectionState {
    pub(crate) anchor: Option<(u16, usize)>,
    pub(crate) end: Option<(u16, usize)>,
    pub(crate) pending_copy: Option<bool>,
    pub(super) mouse_drag_occurred: bool,
    pub(super) suppress_paste_until: Option<Instant>,
}

pub(crate) struct TypewriterState {
    pub(super) chars: VecDeque<char>,
    pub(crate) live: String,
}
