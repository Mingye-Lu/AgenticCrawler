use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

mod control_flow;
mod data;
mod parallel;

#[cfg(test)]
mod tests;

use acrawl_core::{ScriptLimits, ScriptResult, ScriptState, ScriptStatus};
use script::grammar::{Expression, ScriptDefinition, ScriptNode, ALLOWED_TOOLS};
use serde_json::{json, Value};
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

use crate::{BrowserContext, ToolEffect, ToolExecutionError, ToolRegistry};

#[derive(Debug)]
pub enum ScriptExecutionError {
    StepLimitExceeded,
    WallClockTimeout,
    PerStepTimeout,
    Cancelled,
    ToolError(String),
    VariableNotFound(String),
}

impl std::fmt::Display for ScriptExecutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::StepLimitExceeded => write!(f, "script step limit exceeded"),
            Self::WallClockTimeout => write!(f, "script wall-clock timeout exceeded"),
            Self::PerStepTimeout => write!(f, "script step timed out"),
            Self::Cancelled => write!(f, "script cancelled"),
            Self::ToolError(message) => write!(f, "tool error: {message}"),
            Self::VariableNotFound(name) => write!(f, "variable not found: {name}"),
        }
    }
}

impl std::error::Error for ScriptExecutionError {}

pub struct ScriptExecutor {
    browser: BrowserContext,
    state: ScriptState,
    shared_state: Arc<RwLock<ScriptState>>,
    limits: ScriptLimits,
    output_bytes: usize,
    variables: HashMap<String, Value>,
    extracted_data: Vec<Value>,
    pub yielded_data: Arc<RwLock<Vec<Value>>>,
    start_time: Instant,
    step_counter: Arc<AtomicUsize>,
    cancel_token: CancellationToken,
}

impl ScriptExecutor {
    #[must_use]
    pub fn new(
        script_id: String,
        browser: BrowserContext,
        limits: ScriptLimits,
        shared_state: Arc<RwLock<ScriptState>>,
        cancel_token: CancellationToken,
    ) -> Self {
        Self {
            browser,
            state: ScriptState {
                script_id,
                status: ScriptStatus::Pending,
                step: 0,
                total_steps: None,
                current_url: None,
                items_collected: 0,
                elapsed_secs: 0.0,
                errors_caught: 0,
                yielded_data: Vec::new(),
            },
            shared_state,
            limits,
            output_bytes: 0,
            variables: HashMap::new(),
            extracted_data: Vec::new(),
            yielded_data: Arc::new(RwLock::new(Vec::new())),
            start_time: Instant::now(),
            step_counter: Arc::new(AtomicUsize::new(0)),
            cancel_token,
        }
    }

    pub async fn execute(mut self, script: ScriptDefinition) -> ScriptResult {
        self.start_time = Instant::now();
        self.state.status = ScriptStatus::Running;
        self.state.total_steps = Some(script.steps.len());
        self.sync_shared_state();

        let execution_result = async {
            for node in &script.steps {
                self.check_limits()?;
                self.execute_node(node).await?;
                self.state.step = self.step_counter.load(Ordering::Relaxed);
                self.state.elapsed_secs = self.start_time.elapsed().as_secs_f64();
                self.sync_shared_state();
            }
            Ok::<(), ScriptExecutionError>(())
        }
        .await;

        self.state.step = self.step_counter.load(Ordering::Relaxed);
        self.state.elapsed_secs = self.start_time.elapsed().as_secs_f64();

        let (status, error) = match execution_result {
            Ok(()) => (ScriptStatus::Completed, None),
            Err(ScriptExecutionError::Cancelled) => (ScriptStatus::Cancelled, None),
            Err(error) => (ScriptStatus::Failed, Some(error.to_string())),
        };
        self.state.status = status;
        self.sync_shared_state();

        let yielded_data = self
            .yielded_data
            .read()
            .map_or_else(|_| self.state.yielded_data.clone(), |data| data.clone());

        ScriptResult {
            script_id: self.state.script_id.clone(),
            status,
            extracted_data: self.extracted_data,
            yielded_data,
            steps_executed: self.state.step,
            elapsed_secs: self.state.elapsed_secs,
            error,
        }
    }

    async fn execute_node(&mut self, node: &ScriptNode) -> Result<(), ScriptExecutionError> {
        match node {
            ScriptNode::ToolCall {
                tool,
                input,
                output,
            } => {
                self.state.step = self.step_counter.fetch_add(1, Ordering::Relaxed) + 1;
                self.check_limits()?;
                let result = self.execute_tool_call(tool, input, output.as_deref()).await;
                self.sync_shared_state();
                result
            }
            ScriptNode::Assign { variable, value } => {
                let resolved = self.evaluate_expression(value).await?;
                self.variables.insert(variable.clone(), resolved);
                self.sync_shared_state();
                Ok(())
            }
            ScriptNode::Collect { value } => {
                let resolved = self.evaluate_expression(value).await?;
                self.push_extracted(resolved)?;
                self.sync_shared_state();
                Ok(())
            }
            ScriptNode::Yield { value } => {
                let resolved = self.evaluate_expression(value).await?;
                self.push_yielded(resolved)?;
                self.sync_shared_state();
                Ok(())
            }
            ScriptNode::ForLoop {
                variable,
                from,
                to,
                steps,
            } => self.execute_for_loop(variable, from, to, steps).await,
            ScriptNode::ForEach {
                variable,
                iterable,
                steps,
            } => self.execute_for_each(variable, iterable, steps).await,
            ScriptNode::WhileLoop { condition, steps } => {
                self.execute_while_loop(condition, steps).await
            }
            ScriptNode::IfElse {
                condition,
                then_steps,
                else_steps,
            } => {
                self.execute_if_else(condition, then_steps, else_steps.as_deref())
                    .await
            }
            ScriptNode::TryCatch {
                try_steps,
                catch_steps,
                finally_steps,
                error_var,
            } => {
                self.execute_try_catch(
                    try_steps,
                    catch_steps.as_deref(),
                    finally_steps.as_deref(),
                    error_var.as_deref(),
                )
                .await
            }
            ScriptNode::Parallel { branches } => self.execute_parallel(branches).await,
        }
    }

    async fn execute_tool_call(
        &mut self,
        tool: &str,
        input: &Value,
        output_var: Option<&str>,
    ) -> Result<(), ScriptExecutionError> {
        if !ALLOWED_TOOLS.contains(&tool) {
            return Err(ScriptExecutionError::ToolError(format!(
                "tool `{tool}` is not allowed in scripts"
            )));
        }

        let registry = ToolRegistry::new_with_core_tools();
        let resolved_input = self.try_substitute_variables(input)?;

        let tool_effect = if let Some(handler) = registry.get(tool) {
            match handler(&resolved_input) {
                Ok(effect) => effect,
                Err(error) if error.is_requires_async() => {
                    self.execute_async_tool(&registry, tool, &resolved_input)
                        .await?
                }
                Err(error) => return Err(Self::map_tool_error(error)),
            }
        } else {
            return Err(ScriptExecutionError::ToolError(format!(
                "unknown tool: `{tool}`"
            )));
        };

        let output_value = Self::tool_effect_to_value(tool_effect)?;
        self.update_current_url(&output_value);

        if let Some(name) = output_var {
            self.variables.insert(name.to_string(), output_value);
        }

        Ok(())
    }

    #[must_use]
    pub fn substitute_variables(&self, value: &Value) -> Value {
        self.try_substitute_variables(value)
            .unwrap_or_else(|_| value.clone())
    }

    fn check_limits(&self) -> Result<(), ScriptExecutionError> {
        if self.cancel_token.is_cancelled() {
            return Err(ScriptExecutionError::Cancelled);
        }

        let steps = self.step_counter.load(Ordering::Relaxed);

        if steps > self.limits.max_steps {
            return Err(ScriptExecutionError::StepLimitExceeded);
        }

        if self.start_time.elapsed().as_secs() >= self.limits.max_timeout_secs {
            return Err(ScriptExecutionError::WallClockTimeout);
        }

        Ok(())
    }

    fn evaluate_expression<'a>(
        &'a mut self,
        expression: &'a Expression,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<Value, ScriptExecutionError>> + Send + 'a>>
    {
        Box::pin(async move {
            match expression {
                Expression::Literal(value) => self.try_substitute_variables(value),
                Expression::Variable(name) => self
                    .variables
                    .get(name)
                    .cloned()
                    .ok_or_else(|| ScriptExecutionError::VariableNotFound(name.clone())),
                Expression::JsEval(script) => {
                    let output = self.run_execute_js(script).await?;
                    Ok(output.get("result").cloned().unwrap_or(Value::Null))
                }
                Expression::FieldAccess { object, field } => {
                    let value = self.evaluate_expression(object).await?;
                    Ok(value.get(field).cloned().unwrap_or(Value::Null))
                }
                Expression::ArrayIndex { array, index } => {
                    let array_value = self.evaluate_expression(array).await?;
                    let index_value = self.evaluate_expression(index).await?;
                    let index = index_value
                        .as_u64()
                        .and_then(|value| usize::try_from(value).ok())
                        .ok_or_else(|| {
                            ScriptExecutionError::ToolError(
                                "array index must evaluate to a non-negative integer".to_string(),
                            )
                        })?;

                    match array_value {
                        Value::Array(values) => {
                            Ok(values.get(index).cloned().unwrap_or(Value::Null))
                        }
                        _ => Err(ScriptExecutionError::ToolError(
                            "array index target did not evaluate to an array".to_string(),
                        )),
                    }
                }
            }
        })
    }

    async fn run_execute_js(&mut self, script: &str) -> Result<Value, ScriptExecutionError> {
        self.execute_async_tool(
            &ToolRegistry::new_with_core_tools(),
            "execute_js",
            &json!({
                "script": script,
            }),
        )
        .await
        .and_then(Self::tool_effect_to_value)
    }

    async fn execute_async_tool(
        &mut self,
        registry: &ToolRegistry,
        tool: &str,
        input: &Value,
    ) -> Result<ToolEffect, ScriptExecutionError> {
        timeout(
            Duration::from_secs(self.limits.per_step_timeout_secs),
            registry.execute_async(tool, input, &mut self.browser),
        )
        .await
        .map_err(|_| ScriptExecutionError::PerStepTimeout)?
        .map_err(Self::map_tool_error)
    }

    fn try_substitute_variables(&self, value: &Value) -> Result<Value, ScriptExecutionError> {
        match value {
            Value::String(text) => {
                if let Some(variable_name) = Self::variable_name(text) {
                    self.variables.get(variable_name).cloned().ok_or_else(|| {
                        ScriptExecutionError::VariableNotFound(variable_name.to_string())
                    })
                } else {
                    Ok(Value::String(text.clone()))
                }
            }
            Value::Array(values) => values
                .iter()
                .map(|value| self.try_substitute_variables(value))
                .collect::<Result<Vec<_>, _>>()
                .map(Value::Array),
            Value::Object(map) => map
                .iter()
                .map(|(key, value)| Ok((key.clone(), self.try_substitute_variables(value)?)))
                .collect::<Result<serde_json::Map<String, Value>, ScriptExecutionError>>()
                .map(Value::Object),
            _ => Ok(value.clone()),
        }
    }

    fn variable_name(value: &str) -> Option<&str> {
        let candidate = value.strip_prefix('$')?;
        if candidate.is_empty() {
            return None;
        }

        let mut chars = candidate.chars();
        let first = chars.next()?;
        if !(first == '_' || first.is_ascii_alphabetic()) {
            return None;
        }

        if chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric()) {
            Some(candidate)
        } else {
            None
        }
    }

    fn tool_effect_to_value(tool_effect: ToolEffect) -> Result<Value, ScriptExecutionError> {
        match tool_effect {
            ToolEffect::Reply(reply) => {
                Ok(serde_json::from_str(&reply).unwrap_or(Value::String(reply)))
            }
            effect => Err(ScriptExecutionError::ToolError(format!(
                "script executor does not support tool effect: {effect:?}"
            ))),
        }
    }

    fn update_current_url(&mut self, output: &Value) {
        if let Some(url) = output.get("url").and_then(Value::as_str) {
            self.state.current_url = Some(url.to_string());
            return;
        }

        if let Some(url) = output
            .get("page_state")
            .and_then(|page_state| page_state.get("url"))
            .and_then(Value::as_str)
        {
            self.state.current_url = Some(url.to_string());
        }
    }

    #[allow(clippy::needless_pass_by_value)]
    fn map_tool_error(error: ToolExecutionError) -> ScriptExecutionError {
        ScriptExecutionError::ToolError(error.to_string())
    }

    fn sync_shared_state(&self) {
        if let Ok(mut state) = self.shared_state.write() {
            *state = self.state.clone();
        }
    }
}
