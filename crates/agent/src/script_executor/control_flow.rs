use script::grammar::{Expression, ScriptNode};
use serde_json::{Number, Value};
use std::future::Future;
use std::pin::Pin;

use super::{ScriptExecutionError, ScriptExecutor};

impl ScriptExecutor {
    pub(super) fn execute_for_loop<'a>(
        &'a mut self,
        variable: &'a str,
        from: &'a Expression,
        to: &'a Expression,
        steps: &'a [ScriptNode],
    ) -> Pin<Box<dyn Future<Output = Result<(), ScriptExecutionError>> + Send + 'a>> {
        Box::pin(async move {
            let start =
                Self::value_to_i64(self.evaluate_expression(from).await?, "for loop `from`")?;
            let end = Self::value_to_i64(self.evaluate_expression(to).await?, "for loop `to`")?;

            for value in start..end {
                self.check_limits()?;
                self.variables
                    .insert(variable.to_string(), Value::Number(Number::from(value)));

                for step in steps {
                    self.execute_node(step).await?;
                }
            }

            Ok(())
        })
    }

    pub(super) fn execute_for_each<'a>(
        &'a mut self,
        variable: &'a str,
        iterable: &'a Expression,
        steps: &'a [ScriptNode],
    ) -> Pin<Box<dyn Future<Output = Result<(), ScriptExecutionError>> + Send + 'a>> {
        Box::pin(async move {
            match self.evaluate_expression(iterable).await? {
                Value::Array(items) => {
                    for item in items {
                        self.check_limits()?;
                        self.variables.insert(variable.to_string(), item);

                        for step in steps {
                            self.execute_node(step).await?;
                        }
                    }
                }
                Value::Object(map) => {
                    for (key, value) in map {
                        self.check_limits()?;
                        self.variables.insert(
                            variable.to_string(),
                            Value::Object(serde_json::Map::from_iter([
                                (String::from("key"), Value::String(key)),
                                (String::from("value"), value),
                            ])),
                        );

                        for step in steps {
                            self.execute_node(step).await?;
                        }
                    }
                }
                value => {
                    return Err(ScriptExecutionError::ToolError(format!(
                        "for_each iterable must evaluate to an array or object, got {value}"
                    )));
                }
            }

            Ok(())
        })
    }

    pub(super) fn execute_while_loop<'a>(
        &'a mut self,
        condition: &'a Expression,
        steps: &'a [ScriptNode],
    ) -> Pin<Box<dyn Future<Output = Result<(), ScriptExecutionError>> + Send + 'a>> {
        Box::pin(async move {
            while Self::is_truthy(&self.evaluate_expression(condition).await?) {
                self.check_limits()?;

                for step in steps {
                    self.execute_node(step).await?;
                }
            }

            Ok(())
        })
    }

    pub(super) fn execute_if_else<'a>(
        &'a mut self,
        condition: &'a Expression,
        then_steps: &'a [ScriptNode],
        else_steps: Option<&'a [ScriptNode]>,
    ) -> Pin<Box<dyn Future<Output = Result<(), ScriptExecutionError>> + Send + 'a>> {
        Box::pin(async move {
            let steps = if Self::is_truthy(&self.evaluate_expression(condition).await?) {
                then_steps
            } else {
                else_steps.unwrap_or_default()
            };

            for step in steps {
                self.execute_node(step).await?;
            }

            Ok(())
        })
    }

    pub(super) fn execute_try_catch<'a>(
        &'a mut self,
        try_steps: &'a [ScriptNode],
        catch_steps: Option<&'a [ScriptNode]>,
        finally_steps: Option<&'a [ScriptNode]>,
        error_var: Option<&'a str>,
    ) -> Pin<Box<dyn Future<Output = Result<(), ScriptExecutionError>> + Send + 'a>> {
        Box::pin(async move {
            let mut pending_error = None;

            for step in try_steps {
                match self.execute_node(step).await {
                    Ok(()) => {}
                    Err(
                        error @ (ScriptExecutionError::StepLimitExceeded
                        | ScriptExecutionError::WallClockTimeout
                        | ScriptExecutionError::PerStepTimeout
                        | ScriptExecutionError::Cancelled),
                    ) => {
                        pending_error = Some(error);
                        break;
                    }
                    Err(
                        error @ (ScriptExecutionError::ToolError(_)
                        | ScriptExecutionError::VariableNotFound(_)),
                    ) => {
                        self.state.errors_caught += 1;

                        if let Some(name) = error_var {
                            self.variables
                                .insert(name.to_string(), Value::String(error.to_string()));
                        }

                        if let Some(steps) = catch_steps {
                            for step in steps {
                                if let Err(catch_error) = self.execute_node(step).await {
                                    pending_error = Some(catch_error);
                                    break;
                                }
                            }
                        }

                        break;
                    }
                }
            }

            if let Some(steps) = finally_steps {
                for step in steps {
                    self.execute_node(step).await?;
                }
            }

            if let Some(error) = pending_error {
                return Err(error);
            }

            Ok(())
        })
    }

    fn is_truthy(value: &Value) -> bool {
        match value {
            Value::Null => false,
            Value::Bool(boolean) => *boolean,
            Value::Number(number) => number.as_f64().is_some_and(|value| value != 0.0),
            Value::String(text) => !text.is_empty(),
            Value::Array(values) => !values.is_empty(),
            Value::Object(map) => !map.is_empty(),
        }
    }

    #[allow(clippy::needless_pass_by_value)]
    fn value_to_i64(value: Value, context: &str) -> Result<i64, ScriptExecutionError> {
        value.as_i64().ok_or_else(|| {
            ScriptExecutionError::ToolError(format!("{context} must evaluate to an integer"))
        })
    }
}
