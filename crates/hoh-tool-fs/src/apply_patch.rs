//! Multi-file patch tool over the guarded patch executor.

use std::sync::Arc;

use hoh_error::BoxError;
use hoh_llm::ContentBlock;
use hoh_system_prompt::{PromptSection, PromptSectionOrderName, PromptText, SystemPrompt};
use hoh_tools::{
    DefineToolOptions, DefineToolOutput, ToolCallView, ToolExecution, ToolRegistrationOptions,
    ToolResult, define_tool,
};
use serde_json::{Map, Value, json};

use crate::Env;
use crate::apply_patch_output::{
    patch_output_schema, patch_presentation_meta, patch_tool_value, render_patch,
};
use crate::apply_patch_presentation::{present_patch_call, present_patch_result};
use crate::patch::{PatchExecutionRequest, execute_patch};
use crate::sandbox::FsSandboxController;

/// Register the patch tool and scope-aware guidance using the shared mutation controller.
///
/// `env` is the plugin effect scope and filesystem services; `sandbox` is the per-call policy
/// resolver and denial mapper.
pub fn apply_apply_patch_tool(
    env: &Arc<Env>,
    sandbox: &Arc<FsSandboxController>,
) -> Result<(), BoxError> {
    let guidance_env = env.clone();
    env.system_prompt.section(
        &env.ctx,
        PromptSection {
            name: "tool:apply_patch".into(),
            order: SystemPrompt::get_section_order(PromptSectionOrderName::ToolApplyPatch),
            text: PromptText::provider(move |assemble| {
                Ok(if guidance_env.tools.get("apply_patch", assemble.scope.as_ref(), None).is_none() {
                    String::new()
                } else {
                    concat!(
                        "Prefer apply_patch over separate edit calls for changes across files or separated regions. ",
                        "Give every hunk enough surrounding context lines to match one place in the current file. ",
                        "The whole patch is checked first (syntax, hunk matching, missing files) and a rejected patch changes nothing; ",
                        "once applying starts, operations run in order, stop at the first failure, and are not rolled back, so re-read affected files before retrying when needed.",
                    )
                    .to_owned()
                })
            }),
            ..PromptSection::default()
        },
    )?;

    let mut parameters = Map::new();
    parameters.insert(
        "input".into(),
        json!({
            "type": "string",
            "required": true,
            "description": concat!(
                "Patch text: start with *** Begin Patch and end with *** End Patch. ",
                "Use *** Add File: path with + lines; *** Update File: path, optionally followed by *** Move to: path, ",
                "with @@ or @@ context and space-prefixed context, + added, or - removed lines; ",
                "*** End of File anchors the last hunk to the file end. Use *** Delete File: path to delete. ",
                "Paths are relative to the Session working directory.",
            ),
        }),
    );
    if !sandbox.escalation_modes.is_empty() {
        parameters.extend(sandbox.schema_fields());
    }
    let output = DefineToolOutput {
        schema: patch_output_schema(),
        render: Arc::new(|_args, value| vec![ContentBlock::text(render_patch(value))]),
        presentation_meta: Some(Arc::new(|_args, value| patch_presentation_meta(value))),
    };
    let body_env = env.clone();
    let body_sandbox = sandbox.clone();
    let mut options = DefineToolOptions::new(
        "apply_patch",
        concat!(
            "Create, update, delete, or move UTF-8 text files with one patch; ",
            "prefer it over multiple edit calls when a change spans several files or separated regions.",
        ),
        Value::Object(parameters),
        output,
        move |args, exec| {
            let env = body_env.clone();
            let sandbox = body_sandbox.clone();
            Box::pin(async move { execute(&env, &sandbox, &args, &exec).await })
        },
    );
    options.present_call = Some(Arc::new(|args| {
        present_patch_call(args.get("input").and_then(Value::as_str).unwrap_or(""))
            .map(ToolCallView::Diff)
    }));
    options.present_result = Some(Arc::new(|_args, result: &ToolResult| {
        present_patch_result(result)
    }));
    env.tools.register(
        &env.ctx,
        define_tool(options)?,
        ToolRegistrationOptions::default(),
    )?;
    Ok(())
}

async fn execute(
    env: &Env,
    sandbox: &FsSandboxController,
    args: &Value,
    exec: &Arc<ToolExecution>,
) -> Result<Value, BoxError> {
    let sandbox_policy = sandbox.resolve_policy("apply_patch", args, exec).await?;
    let patch = args
        .get("input")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let report = execute_patch(
        env,
        sandbox,
        exec,
        &PatchExecutionRequest {
            patch,
            sandbox_policy,
        },
    )
    .await?;
    patch_tool_value(&report)
}
