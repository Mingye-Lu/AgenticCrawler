use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A script task to be executed. The `script` field contains the raw JSON
/// representation of the script (not the parsed AST) to avoid circular
/// dependencies between core and the script crate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScriptTask {
    pub script: Value,
    pub save_as: Option<String>,
    pub limits: ScriptLimits,
}

/// Runtime state of an executing script.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScriptState {
    pub script_id: String,
    pub status: ScriptStatus,
    pub step: usize,
    pub total_steps: Option<usize>,
    pub current_url: Option<String>,
    pub items_collected: usize,
    pub elapsed_secs: f64,
    pub errors_caught: usize,
    pub yielded_data: Vec<Value>,
}

/// Status of a script execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScriptStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
}

/// Final result of a script execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScriptResult {
    pub script_id: String,
    pub status: ScriptStatus,
    pub extracted_data: Vec<Value>,
    pub yielded_data: Vec<Value>,
    pub steps_executed: usize,
    pub elapsed_secs: f64,
    pub error: Option<String>,
}

/// Execution limits for a script.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScriptLimits {
    pub max_steps: usize,
    pub max_timeout_secs: u64,
    pub max_output_bytes: usize,
    pub max_parallel_branches: usize,
    pub per_step_timeout_secs: u64,
}

/// Parameters for waiting on script execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScriptWaitSpec {
    pub script_ids: Option<Vec<String>>,
}

/// Parameters for cancelling a script.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScriptCancelSpec {
    pub script_id: String,
}

/// Parameters for querying script status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScriptStatusSpec {
    pub script_id: String,
}
