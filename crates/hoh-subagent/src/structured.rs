//! Child-scoped structured-output capture with authoritative result commit.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, MutexGuard},
};

use hoh_error::BoxError;
use hoh_kernel::Context;
use hoh_llm::ContentBlock;
use hoh_system_prompt::{PromptSection, PromptSectionOrderName, SystemPrompt, SystemPromptKey};
use hoh_tools::{
    ToolArgsError, ToolDefinition, ToolExecution, ToolExecutionToken, ToolGuardDenial,
    ToolOutputDefinition, ToolRegistrationOptions, Tools, ToolsResult, validate_json_schema_value,
};
use serde_json::{Map, Value, json};

use crate::error::{SubagentError, SubagentErrorCode};

pub const STRUCTURED_OUTPUT_TOOL: &str = "structured_output";
pub const STRUCTURED_OUTPUT_INSTRUCTION: &str = "When you have your final answer, you MUST report it by calling the `structured_output` tool with arguments matching its parameter schema exactly. Do not finish with a plain text answer: only the tool call counts as your result.";

#[derive(Default)]
struct StructuredState {
    staged: HashMap<ToolExecutionToken, Value>,
    pending: Option<(ToolExecutionToken, Value)>,
    captured: Option<Value>,
}

fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// One structured run's live capture handle.
#[derive(Clone)]
pub struct StructuredAttachment {
    state: Arc<Mutex<StructuredState>>,
}

impl StructuredAttachment {
    pub fn accepts_input(&self) -> bool {
        let state = lock(&self.state);
        state.staged.is_empty() && state.pending.is_none() && state.captured.is_none()
    }

    pub fn captured(&self) -> Option<Value> {
        lock(&self.state).captured.clone()
    }
}

/// Attach the scoped capture tool, prompt instruction, guard and result observer.
pub fn attach_structured_runtime(
    child_ctx: &Context,
    schema: Value,
    can_complete: impl Fn() -> bool + Send + Sync + 'static,
) -> Result<StructuredAttachment, BoxError> {
    let parameters: Map<String, Value> = schema
        .as_object()
        .cloned()
        .ok_or_else(|| "structured output schema must be an object".to_string())?;
    let tools = child_ctx
        .get::<Tools>()
        .ok_or_else(|| "structured output requires the tools service".to_string())?;
    let system_prompt = child_ctx
        .get::<SystemPromptKey>()
        .ok_or_else(|| "structured output requires the systemPrompt service".to_string())?;
    let state = Arc::new(Mutex::new(StructuredState::default()));
    let can_complete = Arc::new(can_complete);

    let execute_state = state.clone();
    let execute_schema = schema.clone();
    let execute_complete = can_complete.clone();
    let execute = Arc::new(move |args: Value, exec: Arc<ToolExecution>| {
        let violations = validate_json_schema_value(&execute_schema, &args, "");
        let state = execute_state.clone();
        let complete = execute_complete.clone();
        Box::pin(async move {
            if !violations.is_empty() {
                return Err(Box::new(ToolArgsError::new(violations)) as BoxError);
            }
            if !complete() {
                return Err(Box::new(SubagentError::error(
                    "Process pending messages before submitting structured output.",
                    SubagentErrorCode::StructuredOutputPendingInput,
                )) as BoxError);
            }
            lock(&state).staged.insert(exec.token, args);
            exec.conclude_turn();
            Ok(json!({ "recorded": true }))
        }) as futures_util::future::BoxFuture<'static, Result<Value, BoxError>>
    });
    let definition = ToolDefinition::new(
        STRUCTURED_OUTPUT_TOOL,
        "Report your final structured result. Call this exactly once, when your answer is complete; the arguments must match this tool's parameter schema exactly.",
        parameters,
        ToolOutputDefinition {
            schema: json!({
                "type": "object",
                "properties": { "recorded": { "type": "boolean", "const": true } },
                "required": ["recorded"],
                "additionalProperties": false
            }),
            render: Arc::new(|_, _| Ok(vec![ContentBlock::text("Structured output recorded.")])),
            presentation_meta: None,
        },
        execute,
    );
    tools.register(child_ctx, definition, ToolRegistrationOptions::default())?;

    system_prompt.section(
        child_ctx,
        PromptSection {
            name: format!("tool:{STRUCTURED_OUTPUT_TOOL}"),
            order: SystemPrompt::get_section_order(PromptSectionOrderName::StructuredOutput),
            text: STRUCTURED_OUTPUT_INSTRUCTION.into(),
            ..PromptSection::default()
        },
    )?;

    let guard_state = state.clone();
    tools.guard(
        child_ctx,
        Arc::new(move |exec| {
            let state = lock(&guard_state);
            Ok((state.captured.is_some() || state.pending.is_some()).then(|| {
                ToolGuardDenial::from(format!(
                    "structured output already recorded: the run is complete, so `{}` is not executed",
                    exec.name
                ))
            }))
        }),
    )?;

    let result_state = state.clone();
    child_ctx.on_contained::<ToolsResult>(move |event| {
        let mut state = lock(&result_state);
        if event.exec.name == STRUCTURED_OUTPUT_TOOL {
            let Some(value) = state.staged.remove(&event.exec.token) else {
                return Ok(());
            };
            if event.result.is_error() {
                return Ok(());
            }
            if let Some(parent) = event.exec.parent {
                if state.captured.is_none() && state.pending.is_none() {
                    state.pending = Some((parent, value));
                }
            } else if state.captured.is_none() {
                state.captured = Some(value);
            }
            return Ok(());
        }
        if state.pending.as_ref().map(|entry| entry.0) != Some(event.exec.token) {
            return Ok(());
        }
        let (_, value) = state.pending.take().expect("pending token was matched");
        if !event.result.is_error() && state.captured.is_none() {
            state.captured = Some(value);
        }
        Ok(())
    })?;

    Ok(StructuredAttachment { state })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_state_is_terminal_and_pending_closes_input() {
        let attachment = StructuredAttachment {
            state: Arc::new(Mutex::new(StructuredState::default())),
        };
        assert!(attachment.accepts_input());
        lock(&attachment.state).captured = Some(json!({ "answer": 42 }));
        assert!(!attachment.accepts_input());
        assert_eq!(attachment.captured(), Some(json!({ "answer": 42 })));
    }

    #[test]
    fn instruction_matches_model_facing_source_text() {
        assert_eq!(
            STRUCTURED_OUTPUT_INSTRUCTION,
            "When you have your final answer, you MUST report it by calling the `structured_output` tool with arguments matching its parameter schema exactly. Do not finish with a plain text answer: only the tool call counts as your result."
        );
    }
}
