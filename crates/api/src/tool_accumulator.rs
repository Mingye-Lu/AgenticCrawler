use std::collections::BTreeMap;

/// A tool call being assembled from streamed content-block events:
/// `(tool_use id, tool name, accumulated JSON input)`.
pub type PendingToolCall = (String, String, String);

/// Assembles tool calls from streamed content-block events, keyed by block
/// index.
///
/// Some providers (the OpenAI-compatible stream in particular) announce every
/// tool call of a turn before closing any of them, so several calls are in
/// flight at once and each argument delta must land on the call it belongs to.
/// [`finish`](Self::finish) returns calls in the order the caller closes them.
#[derive(Debug, Default)]
pub struct ToolCallAccumulator {
    pending: BTreeMap<u32, PendingToolCall>,
}

impl ToolCallAccumulator {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Begins tracking the tool call opened at block `index`.
    pub fn start(&mut self, index: u32, call: PendingToolCall) {
        self.pending.insert(index, call);
    }

    /// Appends a JSON argument fragment to the call at block `index`, if any.
    pub fn push_json(&mut self, index: u32, partial_json: &str) {
        if let Some((_, _, input)) = self.pending.get_mut(&index) {
            input.push_str(partial_json);
        }
    }

    /// Removes and returns the call at block `index`, if one is pending.
    pub fn finish(&mut self, index: u32) -> Option<PendingToolCall> {
        self.pending.remove(&index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(id: &str, name: &str) -> PendingToolCall {
        (id.to_string(), name.to_string(), String::new())
    }

    #[test]
    fn interleaved_calls_keep_their_own_input() {
        let mut acc = ToolCallAccumulator::new();
        acc.start(0, call("a", "read_content"));
        acc.start(1, call("b", "list_resources"));
        acc.push_json(0, "{\"selector\":");
        acc.push_json(1, "{}");
        acc.push_json(0, "\"h1\"}");

        assert_eq!(
            acc.finish(0),
            Some((
                "a".to_string(),
                "read_content".to_string(),
                "{\"selector\":\"h1\"}".to_string()
            ))
        );
        assert_eq!(
            acc.finish(1),
            Some((
                "b".to_string(),
                "list_resources".to_string(),
                "{}".to_string()
            ))
        );
    }

    #[test]
    fn finish_is_none_for_unknown_or_already_finished_index() {
        let mut acc = ToolCallAccumulator::new();
        acc.start(3, call("a", "navigate"));
        assert!(acc.finish(4).is_none());
        assert!(acc.finish(3).is_some());
        assert!(acc.finish(3).is_none());
    }

    #[test]
    fn json_for_unknown_index_is_ignored() {
        let mut acc = ToolCallAccumulator::new();
        acc.start(0, call("a", "navigate"));
        acc.push_json(9, "{\"url\":\"x\"}");
        assert_eq!(
            acc.finish(0).map(|(_, _, input)| input),
            Some(String::new())
        );
    }
}
