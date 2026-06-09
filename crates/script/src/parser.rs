//! Parser for the Autonomous Script Protocol.

use std::collections::HashSet;

use acrawl_core::ScriptLimits;

use crate::{
    error::{ScriptParseError, ValidationError},
    grammar::{Expression, ScriptDefinition, ScriptNode, ALLOWED_TOOLS, SCHEMA_VERSION},
};

pub fn parse_script(json: &serde_json::Value) -> Result<ScriptDefinition, ScriptParseError> {
    let script = serde_json::from_value::<ScriptDefinition>(json.clone())?;

    if script.schema_version != SCHEMA_VERSION {
        return Err(ScriptParseError::WrongSchemaVersion {
            found: script.schema_version,
            expected: SCHEMA_VERSION,
        });
    }

    if !json.is_object() {
        return Err(ScriptParseError::Structural(
            "script root must be a JSON object".to_string(),
        ));
    }

    Ok(script)
}

pub fn validate_script(
    script: &ScriptDefinition,
    limits: &ScriptLimits,
) -> Result<(), Vec<ValidationError>> {
    let mut errors = Vec::new();

    if script.schema_version != SCHEMA_VERSION {
        errors.push(ValidationError::WrongSchemaVersion {
            found: script.schema_version,
            expected: SCHEMA_VERSION,
        });
    }

    match serde_json::to_vec(&script_to_value(script)) {
        Ok(bytes) if bytes.len() > limits.max_script_size_bytes => {
            errors.push(ValidationError::ScriptTooLarge {
                size_bytes: bytes.len(),
                max_bytes: limits.max_script_size_bytes,
            });
        }
        Ok(_) => {}
        Err(error) => errors.push(ValidationError::EmptySteps {
            context: format!("failed to serialize script for size validation: {error}"),
        }),
    }

    if script.steps.is_empty() {
        errors.push(ValidationError::EmptySteps {
            context: "top-level script steps".to_string(),
        });
    }

    let mut scope = HashSet::new();
    validate_steps(
        &script.steps,
        0,
        limits,
        &mut scope,
        "top-level script steps",
        &mut errors,
    );

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

fn script_to_value(script: &ScriptDefinition) -> serde_json::Value {
    serde_json::json!({
        "schema_version": script.schema_version,
        "name": script.name,
        "steps": script.steps.iter().map(node_to_value).collect::<Vec<_>>(),
    })
}

fn node_to_value(node: &ScriptNode) -> serde_json::Value {
    match node {
        ScriptNode::ToolCall {
            tool,
            input,
            output,
        } => serde_json::json!({
            "type": "tool_call",
            "tool": tool,
            "input": input,
            "output": output,
        }),
        ScriptNode::Assign { variable, value } => serde_json::json!({
            "type": "assign",
            "variable": variable,
            "value": expression_to_value(value),
        }),
        ScriptNode::Collect { value } => serde_json::json!({
            "type": "collect",
            "value": expression_to_value(value),
        }),
        ScriptNode::Yield { value } => serde_json::json!({
            "type": "yield",
            "value": expression_to_value(value),
        }),
        ScriptNode::ForLoop {
            variable,
            from,
            to,
            steps,
        } => serde_json::json!({
            "type": "for_loop",
            "variable": variable,
            "from": expression_to_value(from),
            "to": expression_to_value(to),
            "steps": steps.iter().map(node_to_value).collect::<Vec<_>>(),
        }),
        ScriptNode::ForEach {
            variable,
            iterable,
            steps,
        } => serde_json::json!({
            "type": "for_each",
            "variable": variable,
            "iterable": expression_to_value(iterable),
            "steps": steps.iter().map(node_to_value).collect::<Vec<_>>(),
        }),
        ScriptNode::WhileLoop { condition, steps } => serde_json::json!({
            "type": "while_loop",
            "condition": expression_to_value(condition),
            "steps": steps.iter().map(node_to_value).collect::<Vec<_>>(),
        }),
        ScriptNode::IfElse {
            condition,
            then_steps,
            else_steps,
        } => serde_json::json!({
            "type": "if_else",
            "condition": expression_to_value(condition),
            "then_steps": then_steps.iter().map(node_to_value).collect::<Vec<_>>(),
            "else_steps": else_steps.as_ref().map(|steps| steps.iter().map(node_to_value).collect::<Vec<_>>()),
        }),
        ScriptNode::TryCatch {
            try_steps,
            catch_steps,
            finally_steps,
            error_var,
        } => serde_json::json!({
            "type": "try_catch",
            "try_steps": try_steps.iter().map(node_to_value).collect::<Vec<_>>(),
            "catch_steps": catch_steps.as_ref().map(|steps| steps.iter().map(node_to_value).collect::<Vec<_>>()),
            "finally_steps": finally_steps.as_ref().map(|steps| steps.iter().map(node_to_value).collect::<Vec<_>>()),
            "error_var": error_var,
        }),
        ScriptNode::Parallel { branches } => serde_json::json!({
            "type": "parallel",
            "branches": branches
                .iter()
                .map(|branch| branch.iter().map(node_to_value).collect::<Vec<_>>())
                .collect::<Vec<_>>(),
        }),
    }
}

fn expression_to_value(expression: &Expression) -> serde_json::Value {
    match expression {
        Expression::Literal(value) => serde_json::json!({
            "kind": "literal",
            "value": value,
        }),
        Expression::Variable(name) => serde_json::json!({
            "kind": "variable",
            "value": name,
        }),
        Expression::JsEval(code) => serde_json::json!({
            "kind": "js_eval",
            "value": code,
        }),
        Expression::FieldAccess { object, field } => serde_json::json!({
            "kind": "field_access",
            "object": expression_to_value(object),
            "field": field,
        }),
        Expression::ArrayIndex { array, index } => serde_json::json!({
            "kind": "array_index",
            "array": expression_to_value(array),
            "index": expression_to_value(index),
        }),
    }
}

fn validate_steps(
    steps: &[ScriptNode],
    depth: usize,
    limits: &ScriptLimits,
    scope: &mut HashSet<String>,
    context: &str,
    errors: &mut Vec<ValidationError>,
) {
    if steps.is_empty() {
        errors.push(ValidationError::EmptySteps {
            context: context.to_string(),
        });
        return;
    }

    for step in steps {
        validate_node(step, depth, limits, scope, errors);
    }
}

#[allow(clippy::too_many_lines)]
fn validate_node(
    node: &ScriptNode,
    depth: usize,
    limits: &ScriptLimits,
    scope: &mut HashSet<String>,
    errors: &mut Vec<ValidationError>,
) {
    match node {
        ScriptNode::ToolCall { tool, output, .. } => {
            if !ALLOWED_TOOLS.contains(&tool.as_str()) {
                errors.push(ValidationError::UnknownTool { tool: tool.clone() });
            }
            if let Some(output) = output {
                scope.insert(output.clone());
            }
        }
        ScriptNode::Assign { variable, value } => {
            validate_expression(value, scope, &format!("assignment to `{variable}`"), errors);
            scope.insert(variable.clone());
        }
        ScriptNode::Collect { value } => {
            validate_expression(value, scope, "collect expression", errors);
        }
        ScriptNode::Yield { value } => {
            validate_expression(value, scope, "yield expression", errors);
        }
        ScriptNode::ForLoop {
            variable,
            from,
            to,
            steps,
        } => {
            validate_expression(from, scope, &format!("for_loop `{variable}` from"), errors);
            validate_expression(to, scope, &format!("for_loop `{variable}` to"), errors);
            validate_nested_steps(
                steps,
                depth + 1,
                limits,
                scope,
                variable,
                &format!("for_loop `{variable}` body"),
                errors,
            );
        }
        ScriptNode::ForEach {
            variable,
            iterable,
            steps,
        } => {
            validate_expression(
                iterable,
                scope,
                &format!("for_each `{variable}` iterable"),
                errors,
            );
            validate_nested_steps(
                steps,
                depth + 1,
                limits,
                scope,
                variable,
                &format!("for_each `{variable}` body"),
                errors,
            );
        }
        ScriptNode::WhileLoop { condition, steps } => {
            validate_expression(condition, scope, "while_loop condition", errors);
            validate_nested_steps(
                steps,
                depth + 1,
                limits,
                scope,
                "",
                "while_loop body",
                errors,
            );
        }
        ScriptNode::IfElse {
            condition,
            then_steps,
            else_steps,
        } => {
            validate_expression(condition, scope, "if_else condition", errors);
            validate_nested_steps(
                then_steps,
                depth + 1,
                limits,
                scope,
                "",
                "if_else then_steps",
                errors,
            );
            if let Some(else_steps) = else_steps {
                validate_nested_steps(
                    else_steps,
                    depth + 1,
                    limits,
                    scope,
                    "",
                    "if_else else_steps",
                    errors,
                );
            }
        }
        ScriptNode::TryCatch {
            try_steps,
            catch_steps,
            finally_steps,
            error_var,
        } => {
            validate_nested_steps(
                try_steps,
                depth + 1,
                limits,
                scope,
                "",
                "try_catch try_steps",
                errors,
            );

            if let Some(catch_steps) = catch_steps {
                validate_nested_steps(
                    catch_steps,
                    depth + 1,
                    limits,
                    scope,
                    error_var.as_deref().unwrap_or(""),
                    "try_catch catch_steps",
                    errors,
                );
            }

            if let Some(finally_steps) = finally_steps {
                validate_nested_steps(
                    finally_steps,
                    depth + 1,
                    limits,
                    scope,
                    "",
                    "try_catch finally_steps",
                    errors,
                );
            }
        }
        ScriptNode::Parallel { branches } => {
            if branches.len() > limits.max_parallel_branches {
                errors.push(ValidationError::TooManyParallelBranches {
                    branch_count: branches.len(),
                    max: limits.max_parallel_branches,
                });
            }
            if branches.is_empty() {
                errors.push(ValidationError::EmptySteps {
                    context: "parallel branches".to_string(),
                });
            }

            let next_depth = depth + 1;
            if next_depth > limits.max_nesting_depth {
                errors.push(ValidationError::ExcessiveNesting {
                    depth: next_depth,
                    max: limits.max_nesting_depth,
                });
            }

            for (index, branch) in branches.iter().enumerate() {
                let mut branch_scope = scope.clone();
                validate_steps(
                    branch,
                    next_depth,
                    limits,
                    &mut branch_scope,
                    &format!("parallel branch {index}"),
                    errors,
                );
            }
        }
    }
}

fn validate_nested_steps(
    steps: &[ScriptNode],
    next_depth: usize,
    limits: &ScriptLimits,
    scope: &HashSet<String>,
    bound_variable: &str,
    context: &str,
    errors: &mut Vec<ValidationError>,
) {
    if next_depth > limits.max_nesting_depth {
        errors.push(ValidationError::ExcessiveNesting {
            depth: next_depth,
            max: limits.max_nesting_depth,
        });
    }

    let mut nested_scope = scope.clone();
    if !bound_variable.is_empty() {
        nested_scope.insert(bound_variable.to_string());
    }
    validate_steps(
        steps,
        next_depth,
        limits,
        &mut nested_scope,
        context,
        errors,
    );
}

fn validate_expression(
    expression: &Expression,
    scope: &HashSet<String>,
    context: &str,
    errors: &mut Vec<ValidationError>,
) {
    match expression {
        Expression::Literal(_) | Expression::JsEval(_) => {}
        Expression::Variable(name) => {
            if !scope.contains(name) {
                errors.push(ValidationError::UndefinedVariable {
                    name: name.clone(),
                    context: context.to_string(),
                });
            }
        }
        Expression::FieldAccess { object, field } => {
            validate_expression(
                object,
                scope,
                &format!("{context} field access `{field}`"),
                errors,
            );
        }
        Expression::ArrayIndex { array, index } => {
            validate_expression(array, scope, &format!("{context} array access"), errors);
            validate_expression(index, scope, &format!("{context} array index"), errors);
        }
    }
}

#[cfg(test)]
mod tests {
    use acrawl_core::ScriptLimits;
    use serde_json::json;

    use super::{parse_script, validate_script};
    use crate::{
        error::{ScriptParseError, ValidationError},
        grammar::{Expression, ScriptDefinition, ScriptNode, SCHEMA_VERSION},
    };

    fn limits() -> ScriptLimits {
        ScriptLimits {
            max_steps: 100,
            max_timeout_secs: 60,
            max_output_bytes: 1024,
            max_script_size_bytes: 10_000,
            max_parallel_branches: 4,
            max_nesting_depth: 3,
            per_step_timeout_secs: 10,
        }
    }

    #[test]
    fn parse_script_accepts_valid_minimal_script() {
        let json = json!({
            "schema_version": SCHEMA_VERSION,
            "name": "minimal",
            "steps": [
                {
                    "type": "tool_call",
                    "tool": "navigate",
                    "input": {"url": "https://example.com"},
                    "output": "page"
                }
            ]
        });

        let script = parse_script(&json).expect("script should parse");
        assert_eq!(script.schema_version, SCHEMA_VERSION);
        assert_eq!(script.steps.len(), 1);
    }

    #[test]
    fn parse_script_rejects_wrong_schema_version() {
        let json = json!({
            "schema_version": 99,
            "steps": []
        });

        let error = parse_script(&json).expect_err("schema version should fail");
        assert!(matches!(
            error,
            ScriptParseError::WrongSchemaVersion {
                found: 99,
                expected: SCHEMA_VERSION,
            }
        ));
    }

    #[test]
    fn validate_script_accepts_full_featured_script() {
        let script = ScriptDefinition {
            schema_version: SCHEMA_VERSION,
            name: Some("full".to_string()),
            steps: vec![
                ScriptNode::ToolCall {
                    tool: "navigate".to_string(),
                    input: json!({"url": "https://example.com"}),
                    output: Some("page".to_string()),
                },
                ScriptNode::Assign {
                    variable: "items".to_string(),
                    value: Expression::FieldAccess {
                        object: Box::new(Expression::Variable("page".to_string())),
                        field: "results".to_string(),
                    },
                },
                ScriptNode::ForEach {
                    variable: "item".to_string(),
                    iterable: Expression::Variable("items".to_string()),
                    steps: vec![ScriptNode::TryCatch {
                        try_steps: vec![ScriptNode::IfElse {
                            condition: Expression::JsEval("true".to_string()),
                            then_steps: vec![ScriptNode::Collect {
                                value: Expression::FieldAccess {
                                    object: Box::new(Expression::Variable("item".to_string())),
                                    field: "title".to_string(),
                                },
                            }],
                            else_steps: Some(vec![ScriptNode::Yield {
                                value: Expression::Variable("item".to_string()),
                            }]),
                        }],
                        catch_steps: Some(vec![ScriptNode::Yield {
                            value: Expression::Variable("err".to_string()),
                        }]),
                        finally_steps: Some(vec![ScriptNode::Parallel {
                            branches: vec![
                                vec![ScriptNode::ToolCall {
                                    tool: "scroll".to_string(),
                                    input: json!({"direction": "down", "pixels": 100}),
                                    output: None,
                                }],
                                vec![ScriptNode::ToolCall {
                                    tool: "wait".to_string(),
                                    input: json!({"seconds": 1, "selector": "", "state": "attached"}),
                                    output: None,
                                }],
                            ],
                        }]),
                        error_var: Some("err".to_string()),
                    }],
                },
            ],
        };

        assert_eq!(validate_script(&script, &limits()), Ok(()));
    }

    #[test]
    fn validate_script_rejects_unknown_tool() {
        let script = ScriptDefinition {
            schema_version: SCHEMA_VERSION,
            name: None,
            steps: vec![ScriptNode::ToolCall {
                tool: "fork".to_string(),
                input: json!({}),
                output: None,
            }],
        };

        let errors = validate_script(&script, &limits()).expect_err("unknown tool should fail");
        assert!(errors.contains(&ValidationError::UnknownTool {
            tool: "fork".to_string(),
        }));
    }

    #[test]
    fn validate_script_rejects_excessive_nesting() {
        let script = ScriptDefinition {
            schema_version: SCHEMA_VERSION,
            name: None,
            steps: vec![ScriptNode::ForLoop {
                variable: "i".to_string(),
                from: Expression::Literal(json!(0)),
                to: Expression::Literal(json!(1)),
                steps: vec![ScriptNode::WhileLoop {
                    condition: Expression::JsEval("true".to_string()),
                    steps: vec![ScriptNode::IfElse {
                        condition: Expression::JsEval("true".to_string()),
                        then_steps: vec![ScriptNode::Parallel {
                            branches: vec![vec![ScriptNode::ToolCall {
                                tool: "scroll".to_string(),
                                input: json!({"direction": "down", "pixels": 10}),
                                output: None,
                            }]],
                        }],
                        else_steps: None,
                    }],
                }],
            }],
        };

        let errors = validate_script(&script, &limits()).expect_err("nesting should fail");
        assert!(errors.iter().any(|error| matches!(
            error,
            ValidationError::ExcessiveNesting { depth: 4, max: 3 }
        )));
    }

    #[test]
    fn validate_script_rejects_undefined_variable() {
        let script = ScriptDefinition {
            schema_version: SCHEMA_VERSION,
            name: None,
            steps: vec![ScriptNode::Assign {
                variable: "value".to_string(),
                value: Expression::Variable("missing".to_string()),
            }],
        };

        let errors =
            validate_script(&script, &limits()).expect_err("undefined variable should fail");
        assert!(errors.contains(&ValidationError::UndefinedVariable {
            name: "missing".to_string(),
            context: "assignment to `value`".to_string(),
        }));
    }

    #[test]
    fn validate_all_17_browser_tools_accepted() {
        use crate::grammar::ALLOWED_TOOLS;

        let steps: Vec<ScriptNode> = ALLOWED_TOOLS
            .iter()
            .map(|tool| ScriptNode::ToolCall {
                tool: (*tool).to_string(),
                input: json!({}),
                output: None,
            })
            .collect();
        assert_eq!(steps.len(), 17);

        let script = ScriptDefinition {
            schema_version: SCHEMA_VERSION,
            name: Some("all_tools".to_string()),
            steps,
        };

        assert_eq!(validate_script(&script, &limits()), Ok(()));
    }

    #[test]
    fn validate_rejects_agent_control_tools() {
        let agent_tools = [
            "fork",
            "wait_for_subagents",
            "cancel_subagent",
            "subagent_status",
        ];

        for tool_name in &agent_tools {
            let script = ScriptDefinition {
                schema_version: SCHEMA_VERSION,
                name: None,
                steps: vec![ScriptNode::ToolCall {
                    tool: tool_name.to_string(),
                    input: json!({}),
                    output: None,
                }],
            };

            let errors = validate_script(&script, &limits())
                .expect_err(&format!("agent tool `{tool_name}` should be rejected"));
            assert!(
                errors.contains(&ValidationError::UnknownTool {
                    tool: tool_name.to_string(),
                }),
                "expected UnknownTool for `{tool_name}`"
            );
        }
    }

    #[test]
    fn validate_rejects_script_tools() {
        let script_tools = [
            "run_script",
            "script_status",
            "cancel_script",
            "wait_for_scripts",
        ];

        for tool_name in &script_tools {
            let script = ScriptDefinition {
                schema_version: SCHEMA_VERSION,
                name: None,
                steps: vec![ScriptNode::ToolCall {
                    tool: tool_name.to_string(),
                    input: json!({}),
                    output: None,
                }],
            };

            let errors = validate_script(&script, &limits())
                .expect_err(&format!("script tool `{tool_name}` should be rejected"));
            assert!(
                errors.contains(&ValidationError::UnknownTool {
                    tool: tool_name.to_string(),
                }),
                "expected UnknownTool for `{tool_name}`"
            );
        }
    }

    #[test]
    fn parse_script_rejects_version_2() {
        let json = json!({
            "schema_version": 2,
            "steps": []
        });

        let error = parse_script(&json).expect_err("version 2 should be rejected");
        assert!(matches!(
            error,
            ScriptParseError::WrongSchemaVersion {
                found: 2,
                expected: SCHEMA_VERSION,
            }
        ));
    }

    #[test]
    fn validate_rejects_too_many_parallel_branches() {
        let branches: Vec<Vec<ScriptNode>> = (0..5)
            .map(|_| {
                vec![ScriptNode::ToolCall {
                    tool: "scroll".to_string(),
                    input: json!({"direction": "down", "pixels": 100}),
                    output: None,
                }]
            })
            .collect();

        let script = ScriptDefinition {
            schema_version: SCHEMA_VERSION,
            name: None,
            steps: vec![ScriptNode::Parallel { branches }],
        };

        let errors =
            validate_script(&script, &limits()).expect_err("too many branches should fail");
        assert!(errors.contains(&ValidationError::TooManyParallelBranches {
            branch_count: 5,
            max: 4,
        }));
    }

    #[test]
    fn validate_rejects_empty_top_level_steps() {
        let script = ScriptDefinition {
            schema_version: SCHEMA_VERSION,
            name: None,
            steps: vec![],
        };

        let errors = validate_script(&script, &limits()).expect_err("empty steps should fail");
        assert!(errors.contains(&ValidationError::EmptySteps {
            context: "top-level script steps".to_string(),
        }));
    }

    #[test]
    fn validate_rejects_empty_for_loop_body() {
        let script = ScriptDefinition {
            schema_version: SCHEMA_VERSION,
            name: None,
            steps: vec![ScriptNode::ForLoop {
                variable: "i".to_string(),
                from: Expression::Literal(json!(0)),
                to: Expression::Literal(json!(5)),
                steps: vec![],
            }],
        };

        let errors =
            validate_script(&script, &limits()).expect_err("empty for_loop body should fail");
        assert!(errors.contains(&ValidationError::EmptySteps {
            context: "for_loop `i` body".to_string(),
        }));
    }

    #[test]
    fn validate_rejects_empty_parallel_branches() {
        let script = ScriptDefinition {
            schema_version: SCHEMA_VERSION,
            name: None,
            steps: vec![ScriptNode::Parallel { branches: vec![] }],
        };

        let errors =
            validate_script(&script, &limits()).expect_err("empty parallel branches should fail");
        assert!(errors.contains(&ValidationError::EmptySteps {
            context: "parallel branches".to_string(),
        }));
    }

    #[test]
    fn validate_rejects_script_too_large() {
        let tiny_limits = ScriptLimits {
            max_script_size_bytes: 50,
            ..limits()
        };

        let script = ScriptDefinition {
            schema_version: SCHEMA_VERSION,
            name: Some("oversized_script_with_a_very_long_name_to_push_size".to_string()),
            steps: vec![ScriptNode::ToolCall {
                tool: "navigate".to_string(),
                input: json!({"url": "https://example.com/a/very/long/path/to/ensure/size/exceeds/limit"}),
                output: Some("result".to_string()),
            }],
        };

        let errors =
            validate_script(&script, &tiny_limits).expect_err("oversized script should fail");
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ValidationError::ScriptTooLarge { .. })),
            "expected ScriptTooLarge error, got: {errors:?}"
        );
    }

    #[test]
    fn validate_accepts_for_loop() {
        let script = ScriptDefinition {
            schema_version: SCHEMA_VERSION,
            name: None,
            steps: vec![ScriptNode::ForLoop {
                variable: "page".to_string(),
                from: Expression::Literal(json!(1)),
                to: Expression::Literal(json!(10)),
                steps: vec![ScriptNode::ToolCall {
                    tool: "navigate".to_string(),
                    input: json!({"url": "https://example.com"}),
                    output: None,
                }],
            }],
        };

        assert_eq!(validate_script(&script, &limits()), Ok(()));
    }

    #[test]
    fn validate_accepts_while_loop() {
        let script = ScriptDefinition {
            schema_version: SCHEMA_VERSION,
            name: None,
            steps: vec![ScriptNode::WhileLoop {
                condition: Expression::JsEval("page < 10".to_string()),
                steps: vec![ScriptNode::ToolCall {
                    tool: "scroll".to_string(),
                    input: json!({"direction": "down", "pixels": 500}),
                    output: None,
                }],
            }],
        };

        assert_eq!(validate_script(&script, &limits()), Ok(()));
    }

    #[test]
    fn validate_accepts_yield_only_script() {
        let script = ScriptDefinition {
            schema_version: SCHEMA_VERSION,
            name: None,
            steps: vec![ScriptNode::Yield {
                value: Expression::Literal(json!({"status": "done"})),
            }],
        };

        assert_eq!(validate_script(&script, &limits()), Ok(()));
    }

    #[test]
    fn validate_loop_variable_accessible_in_body() {
        let script = ScriptDefinition {
            schema_version: SCHEMA_VERSION,
            name: None,
            steps: vec![ScriptNode::ForLoop {
                variable: "idx".to_string(),
                from: Expression::Literal(json!(0)),
                to: Expression::Literal(json!(3)),
                steps: vec![ScriptNode::Assign {
                    variable: "current".to_string(),
                    value: Expression::Variable("idx".to_string()),
                }],
            }],
        };

        assert_eq!(validate_script(&script, &limits()), Ok(()));
    }

    #[test]
    fn validate_accepts_none_name() {
        let script = ScriptDefinition {
            schema_version: SCHEMA_VERSION,
            name: None,
            steps: vec![ScriptNode::ToolCall {
                tool: "navigate".to_string(),
                input: json!({"url": "https://example.com"}),
                output: None,
            }],
        };

        assert_eq!(validate_script(&script, &limits()), Ok(()));
    }

    #[test]
    fn validate_accepts_assign_and_collect() {
        let script = ScriptDefinition {
            schema_version: SCHEMA_VERSION,
            name: None,
            steps: vec![
                ScriptNode::ToolCall {
                    tool: "read_content".to_string(),
                    input: json!({"selector": "h1"}),
                    output: Some("title".to_string()),
                },
                ScriptNode::Assign {
                    variable: "processed".to_string(),
                    value: Expression::FieldAccess {
                        object: Box::new(Expression::Variable("title".to_string())),
                        field: "text".to_string(),
                    },
                },
                ScriptNode::Collect {
                    value: Expression::Variable("processed".to_string()),
                },
            ],
        };

        assert_eq!(validate_script(&script, &limits()), Ok(()));
    }

    #[test]
    fn validate_accepts_for_each_standalone() {
        let script = ScriptDefinition {
            schema_version: SCHEMA_VERSION,
            name: None,
            steps: vec![
                ScriptNode::ToolCall {
                    tool: "list_resources".to_string(),
                    input: json!({}),
                    output: Some("links".to_string()),
                },
                ScriptNode::ForEach {
                    variable: "link".to_string(),
                    iterable: Expression::Variable("links".to_string()),
                    steps: vec![ScriptNode::Collect {
                        value: Expression::Variable("link".to_string()),
                    }],
                },
            ],
        };

        assert_eq!(validate_script(&script, &limits()), Ok(()));
    }

    #[test]
    fn validate_accepts_if_else_standalone() {
        let script = ScriptDefinition {
            schema_version: SCHEMA_VERSION,
            name: None,
            steps: vec![ScriptNode::IfElse {
                condition: Expression::JsEval("items.length > 0".to_string()),
                then_steps: vec![ScriptNode::ToolCall {
                    tool: "click".to_string(),
                    input: json!({"selector": ".next"}),
                    output: None,
                }],
                else_steps: Some(vec![ScriptNode::Yield {
                    value: Expression::Literal(json!("no items")),
                }]),
            }],
        };

        assert_eq!(validate_script(&script, &limits()), Ok(()));
    }

    #[test]
    fn validate_accepts_try_catch_standalone() {
        let script = ScriptDefinition {
            schema_version: SCHEMA_VERSION,
            name: None,
            steps: vec![ScriptNode::TryCatch {
                try_steps: vec![ScriptNode::ToolCall {
                    tool: "click".to_string(),
                    input: json!({"selector": ".submit"}),
                    output: None,
                }],
                catch_steps: Some(vec![ScriptNode::ToolCall {
                    tool: "screenshot".to_string(),
                    input: json!({}),
                    output: None,
                }]),
                finally_steps: None,
                error_var: Some("err".to_string()),
            }],
        };

        assert_eq!(validate_script(&script, &limits()), Ok(()));
    }

    #[test]
    fn validate_accepts_parallel_standalone() {
        let script = ScriptDefinition {
            schema_version: SCHEMA_VERSION,
            name: None,
            steps: vec![ScriptNode::Parallel {
                branches: vec![
                    vec![ScriptNode::ToolCall {
                        tool: "navigate".to_string(),
                        input: json!({"url": "https://a.com"}),
                        output: None,
                    }],
                    vec![ScriptNode::ToolCall {
                        tool: "navigate".to_string(),
                        input: json!({"url": "https://b.com"}),
                        output: None,
                    }],
                ],
            }],
        };

        assert_eq!(validate_script(&script, &limits()), Ok(()));
    }
}
