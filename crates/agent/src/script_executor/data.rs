//! Data accumulation and variable management for script execution.
//!
//! This module documents and provides helper methods for the data model used by the script executor.
//!
//! ## Data Model
//!
//! The script executor maintains several data structures during execution:
//!
//! ### Variables (`HashMap<String, Value>`)
//! - Stores named values that can be referenced in expressions using `$varname` syntax.
//! - Populated by:
//!   - `Assign` nodes: `{ "type": "assign", "variable": "name", "value": {...} }`
//!   - `ToolCall` output capture: when a tool call has an `output` field, its result is stored as a variable
//!   - Loop variables: `ForLoop`, `ForEach`, and `TryCatch` bind loop/error variables
//! - Variable substitution is recursive: arrays and objects are traversed, and any string value
//!   matching the pattern `$varname` is replaced with the variable's value.
//! - Undefined variable references cause `VariableNotFound` errors.
//!
//! ### Extracted Data (`Vec<Value>`)
//! - Accumulates values collected via `Collect` nodes.
//! - Each `Collect` node evaluates an expression and appends the result to this vector.
//! - Returned in `ScriptResult.extracted_data` when execution completes.
//! - Represents the primary output of a data-extraction script.
//!
//! ### Yielded Data (`Arc<RwLock<Vec<Value>>>`)
//! - Stores values emitted via `Yield` nodes.
//! - Wrapped in `Arc<RwLock<>>` to allow concurrent reads during execution (e.g., via `script_status` queries).
//! - Each `Yield` node:
//!   1. Evaluates its expression
//!   2. Appends to both `state.yielded_data` (for the final result) and `self.yielded_data` (for concurrent access)
//! - Returned in `ScriptResult.yielded_data` when execution completes.
//! - Useful for streaming results or monitoring progress during long-running scripts.
//!
//! ### Tool Output Capture
//! - When a `ToolCall` node has an `output` field (e.g., `"output": "result"`), the tool's result
//!   is automatically stored in the variables map under that name.
//! - The tool result is converted from `ToolEffect::Reply` to a `Value` via JSON deserialization.
//! - Subsequent expressions can reference this value using `$result` syntax.
//!
//! ## Expression Evaluation
//!
//! Expressions are evaluated recursively and support:
//! - **Literal**: JSON values (strings, numbers, objects, arrays)
//! - **Variable**: `{ "kind": "variable", "name": "varname" }` → looks up `$varname` in the variables map
//! - **`JsEval`**: `{ "kind": "js_eval", "script": "..." }` → executes JavaScript in the browser context
//! - **`FieldAccess`**: `{ "kind": "field_access", "object": {...}, "field": "key" }` → accesses object properties
//! - **`ArrayIndex`**: `{ "kind": "array_index", "array": {...}, "index": {...} }` → accesses array elements by index
//!
//! ## Variable Substitution Pattern
//!
//! When a string value exactly matches `$varname` (where `varname` is a valid identifier),
//! it is replaced with the variable's value. This allows tool inputs to reference variables:
//!
//! ```json
//! {
//!   "type": "tool_call",
//!   "tool": "navigate",
//!   "input": { "url": "$base_url" },
//!   "output": "page_content"
//! }
//! ```
//!
//! If `base_url` is `"https://example.com"`, the input becomes `{ "url": "https://example.com" }`.

use super::ScriptExecutor;
use serde_json::Value;

#[allow(dead_code)]
impl ScriptExecutor {
    /// Store a variable in the executor's variable map.
    ///
    /// # Arguments
    /// * `name` - The variable name (without the `$` prefix)
    /// * `value` - The value to store
    ///
    /// # Example
    /// ```ignore
    /// executor.store_variable("page_title".to_string(), Value::String("Example".to_string()));
    /// // Later: expressions can reference $page_title
    /// ```
    pub(super) fn store_variable(&mut self, name: String, value: Value) {
        self.variables.insert(name, value);
    }

    /// Push a value to the extracted data collection.
    ///
    /// Called by `Collect` nodes to accumulate extracted values.
    ///
    /// # Arguments
    /// * `value` - The value to append to `extracted_data`
    ///
    /// # Example
    /// ```ignore
    /// executor.push_extracted(Value::String("item 1".to_string()));
    /// executor.push_extracted(Value::String("item 2".to_string()));
    /// // Later: ScriptResult.extracted_data contains both values
    /// ```
    pub(super) fn push_extracted(&mut self, value: Value) {
        self.extracted_data.push(value);
        self.state.items_collected = self.extracted_data.len();
    }

    /// Push a value to the yielded data collection.
    ///
    /// Called by `Yield` nodes to emit values for concurrent access.
    /// Updates both the state's `yielded_data` and the `Arc<RwLock<>>` for concurrent reads.
    ///
    /// # Arguments
    /// * `value` - The value to yield
    ///
    /// # Errors
    /// Returns a tool error if the `RwLock` is poisoned (should not happen in normal operation).
    ///
    /// # Example
    /// ```ignore
    /// executor.push_yielded(Value::String("progress update".to_string()))?;
    /// // Value is now available for concurrent reads via script_status
    /// ```
    pub(super) fn push_yielded(&mut self, value: Value) -> Result<(), String> {
        self.state.yielded_data.push(value.clone());
        let mut yielded_data = self
            .yielded_data
            .write()
            .map_err(|error| format!("yield buffer lock poisoned: {error}"))?;
        yielded_data.push(value);
        Ok(())
    }

    /// Get a reference to the current variables map.
    ///
    /// Useful for debugging or inspecting the current variable state.
    pub(super) fn variables(&self) -> &std::collections::HashMap<String, Value> {
        &self.variables
    }

    /// Get a reference to the extracted data collection.
    ///
    /// Useful for debugging or inspecting collected values during execution.
    pub(super) fn extracted_data(&self) -> &[Value] {
        &self.extracted_data
    }
}
