//! Grammar definitions for the Autonomous Script Protocol.

use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: u32 = 1;

pub const ALLOWED_TOOLS: &[&str] = &[
    "navigate",
    "click",
    "click_at",
    "fill_form",
    "page_map",
    "read_content",
    "screenshot",
    "go_back",
    "scroll",
    "wait",
    "select_option",
    "execute_js",
    "hover",
    "press_key",
    "switch_tab",
    "list_resources",
    "save_file",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScriptDefinition {
    pub schema_version: u32,
    pub name: Option<String>,
    pub steps: Vec<ScriptNode>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ScriptNode {
    ToolCall {
        tool: String,
        input: serde_json::Value,
        output: Option<String>,
    },
    Assign {
        variable: String,
        value: Expression,
    },
    Collect {
        value: Expression,
    },
    Yield {
        value: Expression,
    },
    ForLoop {
        variable: String,
        from: Expression,
        to: Expression,
        steps: Vec<ScriptNode>,
    },
    ForEach {
        variable: String,
        iterable: Expression,
        steps: Vec<ScriptNode>,
    },
    WhileLoop {
        condition: Expression,
        steps: Vec<ScriptNode>,
    },
    IfElse {
        condition: Expression,
        then_steps: Vec<ScriptNode>,
        else_steps: Option<Vec<ScriptNode>>,
    },
    TryCatch {
        try_steps: Vec<ScriptNode>,
        catch_steps: Option<Vec<ScriptNode>>,
        finally_steps: Option<Vec<ScriptNode>>,
        error_var: Option<String>,
    },
    Parallel {
        branches: Vec<Vec<ScriptNode>>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Expression {
    Literal(serde_json::Value),
    Variable(String),
    JsEval(String),
    FieldAccess {
        object: Box<Expression>,
        field: String,
    },
    ArrayIndex {
        array: Box<Expression>,
        index: Box<Expression>,
    },
}
