//! Model-facing full-file write. It obtains an optional intent from the single policy slot, calls
//! `ctx.fs.write_text` without a stat, then records the resulting version; no policy means an
//! unconditional atomic create-or-overwrite.

use std::sync::Arc;

use hoh_error::BoxError;
use hoh_fs::{FsActor, FsWriteIntent, FsWriteIntentEvent, FsWriteOperation, FsWriteOutcome};
use hoh_llm::ContentBlock;
use hoh_system_prompt::{PromptSection, PromptSectionOrderName, PromptText, SystemPrompt};
use hoh_tools::{
    DefineToolOptions, DefineToolOutput, DiffCallView, DiffResultView, FileDiff, FileLocation,
    ToolArgsError, ToolCallView, ToolExecution, ToolRegistrationOptions, ToolResult,
    ToolResultView, define_tool,
};
use serde_json::{Map, Value, json};

use crate::Env;
use crate::diff::{compute_hunk_diffs, diffs_from_meta, path_from_meta};
use crate::error::remediate_caught;
use crate::mutation_result::mutation_result;
use crate::sandbox::FsSandboxController;
use crate::session_cwd::session_resolve_options;

/// The validated arguments of `write`.
pub struct WriteInput {
    pub file_path: String,
    pub content: String,
}

fn string_arg<'a>(args: &'a Value, name: &str) -> &'a str {
    args.get(name).and_then(Value::as_str).unwrap_or("")
}

/// Validate value constraints the schema DSL can't express: only a non-blank `file_path` — an empty
/// `content` is legitimate (it writes an empty file). `args` are the schema-validated raw tool
/// arguments; `content` passes through untouched.
pub fn parse_write_args(args: &Value) -> Result<WriteInput, BoxError> {
    let file_path = string_arg(args, "file_path");
    if hoh_util_values::js_trim(file_path).is_empty() {
        return Err(ToolArgsError::new(vec!["file_path must be a non-empty string".into()]).into());
    }
    Ok(WriteInput {
        file_path: file_path.to_owned(),
        content: string_arg(args, "content").to_owned(),
    })
}

/// Format a write outcome as one model-facing text block body. `display_path` is the
/// backend-resolved path rendered in the envelope's `<path>` element; `operation` selects the
/// Created/Updated wording. No file content is echoed back.
pub fn format_write_output(display_path: &str, operation: FsWriteOperation) -> String {
    let verb = if operation == FsWriteOperation::Create {
        "Created"
    } else {
        "Updated"
    };
    format!("<path>{display_path}</path>\n<type>file</type>\n<content>\n{verb} file\n</content>")
}

/// Register the `write` tool and its scope-aware system-prompt guidance. `sandbox` is the shared
/// sandbox-escalation API (advertisement, mode stamping, denial mapping).
pub fn apply_write_tool(
    env: &Arc<Env>,
    sandbox: &Arc<FsSandboxController>,
) -> Result<(), BoxError> {
    let guidance_env = env.clone();
    env.system_prompt.section(
        &env.ctx,
        PromptSection {
            name: "tool:write".into(),
            order: SystemPrompt::get_section_order(PromptSectionOrderName::ToolWrite),
            text: PromptText::provider(move |assemble| {
                let tools = &guidance_env.tools;
                let scope = assemble.scope.as_ref();
                if tools.get("write", scope, None).is_none() {
                    return Ok(String::new());
                }
                let mut text =
                    String::from("Read an existing file before overwriting it with write");
                if tools.get("edit", scope, None).is_some() {
                    text.push_str(" and prefer edit for targeted changes");
                }
                text.push('.');
                Ok(text)
            }),
            ..PromptSection::default()
        },
    )?;

    let mut parameters = Map::new();
    parameters.insert(
        "file_path".into(),
        json!({
            "type": "string", "required": true,
            "description": "Path to write, resolved by the filesystem backend. Provide `file_path` before `content` in the arguments.",
        }),
    );
    parameters.insert(
        "content".into(),
        json!({ "type": "string", "required": true, "description": "Full UTF-8 text content to write." }),
    );
    if !sandbox.escalation_modes.is_empty() {
        parameters.extend(sandbox.schema_fields());
    }
    let output = DefineToolOutput {
        schema: json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "path": { "type": "string", "required": true, "description": "Canonical absolute path in the filesystem execution world." },
                "operation": { "type": "string", "required": true, "enum": ["create", "update"] },
                "before": { "required": true, "oneOf": [{ "type": "string" }, { "type": "null" }] },
                "after": { "type": "string", "required": true },
            },
        }),
        render: Arc::new(|_args, value| {
            let operation = if value["operation"] == "create" {
                FsWriteOperation::Create
            } else {
                FsWriteOperation::Update
            };
            vec![ContentBlock::text(format_write_output(
                string_arg(value, "path"),
                operation,
            ))]
        }),
        presentation_meta: Some(Arc::new(|_args, value| {
            let path = string_arg(value, "path");
            let diffs = match value["before"].as_str() {
                None => Vec::new(),
                Some(before) => compute_hunk_diffs(path, before, string_arg(value, "after")),
            };
            json!({ "operation": value["operation"], "path": path, "diffs": diffs })
        })),
    };
    let body_env = env.clone();
    let body_sandbox = sandbox.clone();
    let mut options = DefineToolOptions::new(
        "write",
        "Create or fully replace a UTF-8 text file.",
        Value::Object(parameters),
        output,
        move |args, exec| {
            let env = body_env.clone();
            let sandbox = body_sandbox.clone();
            Box::pin(async move { execute(&env, &sandbox, &args, &exec).await })
        },
    );
    options.present_call = Some(Arc::new(|args| {
        let path = string_arg(args, "file_path");
        Some(ToolCallView::Diff(DiffCallView {
            title: format!("Write {path}"),
            diffs: vec![FileDiff {
                path: path.to_owned(),
                old_text: None,
                new_text: string_arg(args, "content").to_owned(),
            }],
            locations: Some(vec![FileLocation {
                path: path.to_owned(),
                line: None,
            }]),
        }))
    }));
    // Result-time display repeats the diff because completed views replace the pending view.
    // Overwrites use applied metadata; creates and identical overwrites use the replay-safe args
    // fallback.
    options.present_result = Some(Arc::new(|args, result: &ToolResult| {
        if result.is_error {
            return None;
        }
        let path = string_arg(args, "file_path");
        let diffs = diffs_from_meta(result.meta.as_ref()).unwrap_or_else(|| {
            vec![FileDiff {
                path: path_from_meta(result.meta.as_ref()).unwrap_or_else(|| path.to_owned()),
                old_text: None,
                new_text: string_arg(args, "content").to_owned(),
            }]
        });
        Some(ToolResultView::Diff(DiffResultView {
            title: Some(format!("Write {path}")),
            diffs,
        }))
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
    let input = parse_write_args(args)?;
    // Resolve the per-call sandbox policy BEFORE anything executes; an escalating call fails with
    // its distinct text on any non-grant.
    let sandbox_policy = sandbox.resolve_policy("write", args, exec).await?;
    let options = session_resolve_options(env, exec).await?;
    let target = env.fs.resolve(&input.file_path, options).await?;
    // Single-slot decision: the policy plugin produces createIfAbsent/replaceIfVersion; the bare
    // default is `None` (unconditional). No stat.
    let intent: Option<FsWriteIntent> = env
        .ctx
        .waterfall(
            FsWriteIntentEvent {
                target: target.clone(),
                actor: Some(exec.clone() as FsActor),
            },
            |_| async { Ok(None) },
        )
        .await?;
    let outcome: FsWriteOutcome = match env
        .fs
        .write_text(
            &target,
            &input.content,
            intent,
            Some(exec.signal()),
            sandbox_policy.clone(),
        )
        .await
    {
        Ok(outcome) => outcome,
        // A sandbox denial becomes the shared [sandbox: …] marker (the model recognizes it from
        // bash); guarded mutation failures receive their stable model-facing diagnostic; anything
        // else passes through.
        Err(error) => {
            return Err(remediate_caught(
                sandbox.map_error(error, sandbox_policy.as_ref()),
                &target.display_path,
            ));
        }
    };
    Ok(mutation_result(
        env,
        &target,
        &outcome,
        &outcome.version,
        exec,
    ))
}
