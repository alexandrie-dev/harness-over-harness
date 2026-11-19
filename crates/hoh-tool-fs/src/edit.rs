//! Model-facing literal edit, unique-match by default. It obtains an optional guard from the single
//! intent slot, calls `ctx.fs.edit_text` without a separate stat, then records the observed version;
//! no policy means an unconditional atomic edit.

use std::sync::Arc;

use hoh_error::BoxError;
use hoh_fs::{FsActor, FsEditIntent, FsEditOutcome, FsEditRequest, FsVersionGuard};
use hoh_llm::ContentBlock;
use hoh_system_prompt::{PromptSection, PromptSectionOrderName, PromptText, SystemPrompt};
use hoh_tools::{
    DefineToolOptions, DefineToolOutput, DiffCallView, DiffResultView, FileDiff, FileLocation,
    ToolArgsError, ToolCallView, ToolExecution, ToolRegistrationOptions, ToolResult,
    ToolResultView, define_tool,
};
use serde_json::{Map, Value, json};

use crate::Env;
use crate::diff::{compute_hunk_diffs, diffs_from_meta};
use crate::error::remediate_caught;
use crate::mutation_result::mutation_result;
use crate::sandbox::FsSandboxController;
use crate::session_cwd::session_resolve_options;

/// Validated `edit` arguments after defaulting.
pub struct EditInput {
    pub file_path: String,
    pub old_string: String,
    pub new_string: String,
    pub replace_all: bool,
}

fn string_arg<'a>(args: &'a Value, name: &str) -> &'a str {
    args.get(name).and_then(Value::as_str).unwrap_or("")
}

/// Validate value constraints the schema DSL can't express: a non-blank `file_path`, a non-empty
/// `old_string`, and `old_string != new_string` (an equal pair would be a guaranteed no-op edit).
/// `args` are the schema-validated raw tool arguments; `replace_all` defaults to false.
pub fn parse_edit_args(args: &Value) -> Result<EditInput, BoxError> {
    let file_path = string_arg(args, "file_path");
    let old_string = string_arg(args, "old_string");
    let new_string = string_arg(args, "new_string");
    if hoh_util_values::js_trim(file_path).is_empty() {
        return Err(ToolArgsError::new(vec!["file_path must be a non-empty string".into()]).into());
    }
    if old_string.is_empty() {
        return Err(
            ToolArgsError::new(vec!["old_string must be a non-empty string".into()]).into(),
        );
    }
    if old_string == new_string {
        return Err(
            ToolArgsError::new(vec!["old_string and new_string must differ".into()]).into(),
        );
    }
    Ok(EditInput {
        file_path: file_path.to_owned(),
        old_string: old_string.to_owned(),
        new_string: new_string.to_owned(),
        replace_all: args
            .get("replace_all")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

/// Format an edit success (single-match or replace-all) as a Claude-style model-facing message.
/// `display_path` is the backend-resolved path shown to the model; `replace_all` selects the
/// all-occurrences wording over the single-replacement one.
pub fn format_edit_output(display_path: &str, replace_all: bool) -> String {
    if replace_all {
        format!(
            "The file {display_path} has been updated. All occurrences were successfully replaced."
        )
    } else {
        format!("The file {display_path} has been updated successfully.")
    }
}

/// Register the `edit` tool and its scope-aware system-prompt guidance. `sandbox` is the shared
/// sandbox-escalation API (advertisement, mode stamping, denial mapping).
pub fn apply_edit_tool(env: &Arc<Env>, sandbox: &Arc<FsSandboxController>) -> Result<(), BoxError> {
    let guidance_env = env.clone();
    env.system_prompt.section(
        &env.ctx,
        PromptSection {
            name: "tool:edit".into(),
            order: SystemPrompt::get_section_order(PromptSectionOrderName::ToolEdit),
            text: PromptText::provider(move |assemble| {
                Ok(if guidance_env.tools.get("edit", assemble.scope.as_ref(), None).is_none() {
                    String::new()
                } else {
                    "Read a file before editing it unless you just created or edited it in this session."
                        .to_owned()
                })
            }),
            ..PromptSection::default()
        },
    )?;

    let mut parameters = Map::new();
    parameters.insert(
        "file_path".into(),
        json!({
            "type": "string", "required": true,
            "description": "Path to edit, resolved by the filesystem backend. Provide `file_path` before `old_string` and `new_string` in the arguments.",
        }),
    );
    parameters.insert(
        "old_string".into(),
        json!({ "type": "string", "required": true, "description": "Literal text to replace." }),
    );
    parameters.insert(
        "new_string".into(),
        json!({ "type": "string", "required": true, "description": "Literal replacement text. Use an empty string to delete the match." }),
    );
    parameters.insert(
        "replace_all".into(),
        json!({ "type": "boolean", "description": "Replace all matches. Defaults to false; when false, old_string must appear exactly once." }),
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
                "before": { "type": "string", "required": true },
                "after": { "type": "string", "required": true },
            },
        }),
        render: Arc::new(|args, value| {
            let replace_all = args
                .get("replace_all")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            vec![ContentBlock::text(format_edit_output(
                string_arg(value, "path"),
                replace_all,
            ))]
        }),
        presentation_meta: Some(Arc::new(|_args, value| {
            let path = string_arg(value, "path");
            json!({
                "path": path,
                "diffs": compute_hunk_diffs(path, string_arg(value, "before"), string_arg(value, "after")),
            })
        })),
    };
    let body_env = env.clone();
    let body_sandbox = sandbox.clone();
    let mut options = DefineToolOptions::new(
        "edit",
        "Edit an existing UTF-8 text file by replacing literal text.",
        Value::Object(parameters),
        output,
        move |args, exec| {
            let env = body_env.clone();
            let sandbox = body_sandbox.clone();
            Box::pin(async move { execute(&env, &sandbox, &args, &exec).await })
        },
    );
    // Pure display: a diff card of the literal replacement (old_string → new_string), derived from
    // the call args. `old_text: old_string || null` matches claude-agent-acp's Edit arm; new_string
    // is a required arg here, so it maps straight to new_text.
    options.present_call = Some(Arc::new(|args| {
        let path = string_arg(args, "file_path");
        let old = string_arg(args, "old_string");
        Some(ToolCallView::Diff(DiffCallView {
            title: format!("Edit {path}"),
            diffs: vec![FileDiff {
                path: path.to_owned(),
                old_text: (!old.is_empty()).then(|| old.to_owned()),
                new_text: string_arg(args, "new_string").to_owned(),
            }],
            locations: Some(vec![FileLocation {
                path: path.to_owned(),
                line: None,
            }]),
        }))
    }));
    // Applied metadata replaces the call-time snippet; errors or malformed replay metadata use the
    // generic result rendering.
    options.present_result = Some(Arc::new(|args, result: &ToolResult| {
        if result.is_error {
            return None;
        }
        let diffs = diffs_from_meta(result.meta.as_ref())?;
        Some(ToolResultView::Diff(DiffResultView {
            title: Some(format!("Edit {}", string_arg(args, "file_path"))),
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
    let input = parse_edit_args(args)?;
    // Resolve the per-call sandbox policy BEFORE anything executes.
    let sandbox_policy = sandbox.resolve_policy("edit", args, exec).await?;
    let options = session_resolve_options(env, exec).await?;
    let target = env.fs.resolve(&input.file_path, options).await?;
    // Single-slot decision: the policy plugin returns a version guard or fails with FS_NOT_OBSERVED;
    // the bare default is `None` (unconditional edit). No stat. The intent slot itself can fail with
    // FS_NOT_OBSERVED for an unread target, so it sits inside the guarded block: both that refusal
    // and the provider's guarded-mutation failure get the model-facing remedy below.
    let outcome: Result<FsEditOutcome, BoxError> = async {
        let intent: Option<FsVersionGuard> = env
            .ctx
            .waterfall(
                FsEditIntent {
                    target: target.clone(),
                    actor: Some(exec.clone() as FsActor),
                },
                |_| async { Ok(None) },
            )
            .await?;
        env.fs
            .edit_text(
                &target,
                FsEditRequest {
                    old_string: input.old_string.clone(),
                    new_string: input.new_string.clone(),
                    replace_all: input.replace_all,
                },
                intent,
                Some(exec.signal()),
                sandbox_policy.clone(),
            )
            .await
    }
    .await;
    match outcome {
        Ok(outcome) => Ok(mutation_result(
            env,
            &target,
            &outcome,
            &outcome.version,
            exec,
        )),
        // A sandbox denial becomes the shared [sandbox: …] marker; guarded mutation failures receive
        // their stable model-facing diagnostic; anything else passes through.
        Err(error) => Err(remediate_caught(
            sandbox.map_error(error, sandbox_policy.as_ref()),
            &target.display_path,
        )),
    }
}
