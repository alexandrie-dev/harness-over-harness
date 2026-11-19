//! Port of `@deepseek-ai/dsh-tool-str-replace-editor`.
//!
//! Registers the standalone model-facing `str_replace_editor` tool over the mounted filesystem.

use std::{cmp::Ordering, sync::Arc};

use hoh_error::{BoxError, HarnessError};
use hoh_fs::{
    Fs, FsActor, FsEditIntent, FsError, FsErrorCode, FsInfo, FsObservation, FsObserved,
    FsResolveOptions, FsTarget, FsType, FsVersionGuard, FsWriteIntent, FsWriteIntentEvent,
};
use hoh_kernel::{Cfg, Context, Inject, Plugin, Service};
use hoh_llm::ContentBlock;
use hoh_output_retention::truncate_without_splitting_surrogate_pair;
use hoh_sandbox::{SandboxExecutionPolicy, sandbox_denial_marker};
use hoh_sandbox_policy::{SandboxPolicy, SandboxPolicyRequest};
use hoh_tools::{
    DefineToolOptions, DefineToolOutput, DiffCallView, FileDiff, FileLocation, GenericCallView,
    ToolArgsError, ToolCallKind, ToolCallView, ToolExecution, ToolRegistrationOptions, Tools,
    define_tool,
};
use hoh_util_values::{js_number_string, js_trim};
use hoh_util_workspace_path::node_path;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

const TRUNCATED_MESSAGE: &str = "<response clipped><NOTE>To save on context only part of this file has been shown to you. You should retry this tool after you have searched inside the file with `grep -n` in order to find the line numbers of what you are looking for.</NOTE>";

const DEFAULT_DESCRIPTION: &str = r#"Custom editing tool for viewing, creating and editing files
* State is persistent across command calls and discussions with the user
* If `path` is a file, `view` displays the result of applying `cat -n`. If `path` is a directory, `view` lists non-hidden files and directories up to 2 levels deep
* The `create` command cannot be used if the specified `path` already exists as a file
* If a `command` generates a long output, it will be truncated and marked with `<response clipped>`
* A null placeholder for a parameter unused by the selected command is treated as omitted. Required parameters still need values; omit `str_replace.new_str` rather than setting it to null when deleting a match

Notes for using the `str_replace` command:
* The `old_str` parameter should match EXACTLY one or more consecutive lines from the original file. Be mindful of whitespaces!
* If the `old_str` parameter is not unique in the file, the replacement will not be performed. Make sure to include enough context in `old_str` to make it unique
* The `new_str` parameter should contain the edited lines that should replace the `old_str`"#;

const DEFAULT_MAX_OUTPUT_CHARS: f64 = 16_000.0;
const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

fn default_max_output_chars() -> f64 {
    DEFAULT_MAX_OUTPUT_CHARS
}

fn default_description() -> String {
    DEFAULT_DESCRIPTION.to_owned()
}

/// Runtime configuration for [`ToolStrReplaceEditor`].
///
/// Implementation constants are deliberately not part of the package API:
///
/// ```compile_fail
/// use hoh_tool_str_replace_editor::{DEFAULT_DESCRIPTION, TRUNCATED_MESSAGE};
/// ```
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", default)]
pub struct Config {
    /// Maximum returned view characters before clipping. Counted in UTF-16 code units.
    #[serde(default = "default_max_output_chars")]
    pub max_output_chars: f64,
    /// Model-facing tool description.
    #[serde(default = "default_description")]
    pub description: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            max_output_chars: default_max_output_chars(),
            description: default_description(),
        }
    }
}

#[derive(Clone)]
struct Env {
    ctx: Context,
    fs: Service<Fs>,
    tools: Service<Tools>,
    policy: MutationPolicy,
    max_output_chars: u64,
}

#[derive(Clone)]
struct MutationPolicy {
    policy: Option<Service<SandboxPolicy>>,
}

impl MutationPolicy {
    fn new(ctx: &Context, fs: &dyn hoh_fs::FileSystem) -> Result<Self, BoxError> {
        let confined = fs.sandbox_mode().is_some();
        let policy = confined.then(|| ctx.get::<SandboxPolicy>()).flatten();
        if confined && policy.is_none() {
            return Err(
                "tool-str-replace-editor: the mounted filesystem confines but ctx.sandboxPolicy is missing"
                    .into(),
            );
        }
        Ok(Self { policy })
    }

    fn resolve(&self, exec: &ToolExecution) -> Result<Option<SandboxExecutionPolicy>, BoxError> {
        self.policy
            .as_ref()
            .map(|policy| {
                policy.resolve(SandboxPolicyRequest {
                    session: exec.agent.as_ref().map(|agent| agent.session()),
                    mode: None,
                })
            })
            .transpose()
    }

    fn map_error(error: BoxError, policy: Option<&SandboxExecutionPolicy>) -> BoxError {
        let denied = HarnessError::of(error.as_ref()).is_some_and(|typed| {
            typed.is_named(FsError::NAME) && typed.code() == FsErrorCode::SandboxDenied.as_str()
        });
        if !denied {
            return error;
        }
        let Some(policy) = policy else {
            return error;
        };
        Box::new(
            FsError::new(
                sandbox_denial_marker(policy.mode),
                FsErrorCode::SandboxDenied,
            )
            .with_cause(error),
        )
    }
}

fn args_error(message: impl Into<String>) -> BoxError {
    Box::new(ToolArgsError::new(vec![message.into()]))
}

fn maybe_truncate(content: String, max_output_chars: u64) -> String {
    if content.encode_utf16().count() as u64 <= max_output_chars {
        content
    } else {
        format!(
            "{}{TRUNCATED_MESSAGE}",
            truncate_without_splitting_surrogate_pair(&content, max_output_chars)
        )
    }
}

fn utf16_compare(left: &str, right: &str) -> Ordering {
    left.encode_utf16().cmp(right.encode_utf16())
}

fn match_offsets(content: &str, search: &str) -> Vec<usize> {
    content
        .match_indices(search)
        .map(|(offset, _)| offset)
        .collect()
}

fn line_numbers_at(content: &str, offsets: &[usize]) -> Vec<usize> {
    let mut line = 1;
    let mut cursor = 0;
    offsets
        .iter()
        .map(|&offset| {
            line += content.as_bytes()[cursor..offset]
                .iter()
                .filter(|&&byte| byte == b'\n')
                .count();
            cursor = offset;
            line
        })
        .collect()
}

async fn resolve_target(env: &Env, path: &str, exec: &ToolExecution) -> Result<FsTarget, BoxError> {
    if js_trim(path).is_empty() {
        return Err(args_error("path must be a non-empty string"));
    }
    if !node_path::is_absolute(path) {
        return Err(args_error(format!(
            "The path {path} is not an absolute path, it should start with `/`. Maybe you meant /{path}?"
        )));
    }
    env.fs
        .resolve(
            path,
            FsResolveOptions {
                signal: Some(exec.signal()),
                ..FsResolveOptions::default()
            },
        )
        .await
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Command {
    View,
    Create,
    StrReplace,
    Insert,
}

impl Command {
    fn parse(value: &str) -> Self {
        match value {
            "view" => Self::View,
            "create" => Self::Create,
            "str_replace" => Self::StrReplace,
            "insert" => Self::Insert,
            _ => unreachable!("define_tool validates the command enum"),
        }
    }
}

async fn stat_existing(
    env: &Env,
    target: &FsTarget,
    command: Command,
    exec: &Arc<ToolExecution>,
) -> Result<FsInfo, BoxError> {
    let info = env.fs.stat(target, Some(exec.signal())).await?;
    let Some(info) = info else {
        env.ctx.emit(&FsObserved {
            target: target.clone(),
            observation: FsObservation::Absent,
            actor: Some(exec.clone() as FsActor),
        });
        return Err(FsError::new(
            format!(
                "The path {} does not exist. Please provide a valid path.",
                target.display_path
            ),
            FsErrorCode::NotFound,
        )
        .into());
    };
    if info.r#type == FsType::Directory && command != Command::View {
        return Err(FsError::new(
            format!(
                "The path {} is a directory and only the `view` command can be used on directories",
                target.display_path
            ),
            FsErrorCode::NotRegularFile,
        )
        .into());
    }
    Ok(info)
}

fn required_for_command<'a>(
    value: Option<&'a str>,
    parameter: &str,
    command: &str,
    allow_empty: bool,
) -> Result<&'a str, BoxError> {
    let Some(value) = value else {
        return Err(args_error(format!(
            "Parameter `{parameter}` is required for command: {command}"
        )));
    };
    if !allow_empty && value.is_empty() {
        return Err(args_error(format!(
            "Parameter `{parameter}` is empty for command: {command}"
        )));
    }
    Ok(value)
}

fn joined_numbers(values: &[f64]) -> String {
    values
        .iter()
        .map(|value| js_number_string(*value))
        .collect::<Vec<_>>()
        .join(", ")
}

fn format_file_view(
    path: &str,
    content: &str,
    max_output_chars: u64,
    view_range: Option<&[f64]>,
) -> Result<String, BoxError> {
    let all_lines: Vec<&str> = content.split('\n').collect();
    let mut initial_line = 1usize;
    let mut final_line = None;
    let mut prompt = format!(
        "Here's the content of {path} with line numbers (which has a total of {} lines)",
        all_lines.len()
    );
    if let Some(view_range) = view_range {
        if view_range.len() != 2 || view_range.iter().any(|value| value.fract() != 0.0) {
            return Err(args_error(
                "Invalid `view_range`. It should be a list of two integers.",
            ));
        }
        let requested_initial = view_range[0];
        let requested_final = view_range[1];
        if requested_initial < 1.0 || requested_initial > all_lines.len() as f64 {
            return Err(args_error(format!(
                "Invalid `view_range`: [{}]. Its first element `{}` should be within the range of lines of the file: [1, {}]",
                joined_numbers(view_range),
                js_number_string(requested_initial),
                all_lines.len()
            )));
        }
        if requested_final > all_lines.len() as f64 {
            return Err(args_error(format!(
                "Invalid `view_range`: [{}]. Its second element `{}` should be smaller than the number of lines in the file: `{}`",
                joined_numbers(view_range),
                js_number_string(requested_final),
                all_lines.len()
            )));
        }
        if requested_final != -1.0 && requested_final < requested_initial {
            return Err(args_error(format!(
                "Invalid `view_range`: [{}]. Its second element `{}` should be larger or equal than its first `{}`",
                joined_numbers(view_range),
                js_number_string(requested_final),
                js_number_string(requested_initial)
            )));
        }
        initial_line = requested_initial as usize;
        final_line = Some(requested_final);
        prompt.push_str(&format!(
            " with view_range=[{}, {}]",
            js_number_string(requested_initial),
            js_number_string(requested_final)
        ));
    }
    let end = match final_line {
        None | Some(-1.0) => all_lines.len(),
        Some(value) => value as usize,
    };
    let numbered = all_lines[initial_line - 1..end]
        .iter()
        .enumerate()
        .map(|(index, line)| format!("{:>6}  {line}", initial_line + index))
        .collect::<Vec<_>>()
        .join("\n");
    Ok(maybe_truncate(
        format!("{prompt}:\n{numbered}\n"),
        max_output_chars,
    ))
}

fn visit_directory<'a>(
    fs: &'a dyn hoh_fs::FileSystem,
    directory: &'a FsTarget,
    depth: u8,
    signal: hoh_timeout::AbortSignal,
) -> hoh_fs::FsFuture<'a, Vec<String>> {
    Box::pin(async move {
        let entries = fs.list_dir(directory, Some(signal.clone())).await?;
        let mut rows = Vec::new();
        for entry in entries.into_iter().filter(|candidate| {
            !candidate.name.starts_with('.')
                && candidate.name != "node_modules"
                && candidate.name != "__pycache__"
        }) {
            let kind = match entry.r#type {
                FsType::Directory => 'd',
                FsType::File => 'f',
                FsType::Other => '?',
            };
            rows.push(format!("{kind}\t{}", entry.target.display_path));
            if entry.r#type == FsType::Directory && depth < 2 {
                rows.extend(visit_directory(fs, &entry.target, depth + 1, signal.clone()).await?);
            }
        }
        Ok(rows)
    })
}

async fn list_directory(
    env: &Env,
    target: &FsTarget,
    exec: &ToolExecution,
) -> Result<String, BoxError> {
    let mut rows = vec![format!("d\t{}", target.display_path)];
    rows.extend(visit_directory(&*env.fs, target, 1, exec.signal()).await?);
    rows.sort_by(|left, right| {
        let left = left.split_once('\t').map_or("", |(_, path)| path);
        let right = right.split_once('\t').map_or("", |(_, path)| path);
        utf16_compare(left, right)
    });
    let listing = maybe_truncate(format!("{}\n", rows.join("\n")), env.max_output_chars);
    Ok(format!(
        "Here're the files and directories up to 2 levels deep in {}, excluding hidden items, node_modules, and Python cache directories:\n{listing}\n",
        target.display_path
    ))
}

async fn view_path(
    env: &Env,
    path: &str,
    view_range: Option<&[f64]>,
    exec: &Arc<ToolExecution>,
) -> Result<String, BoxError> {
    let target = resolve_target(env, path, exec).await?;
    let info = stat_existing(env, &target, Command::View, exec).await?;
    if info.r#type == FsType::Directory {
        if view_range.is_some() {
            return Err(args_error(
                "The `view_range` parameter is not allowed when `path` points to a directory.",
            ));
        }
        return list_directory(env, &target, exec).await;
    }
    if info.r#type != FsType::File {
        return Err(FsError::new(
            format!(
                "cannot view \"{}\": not a regular file or directory",
                target.display_path
            ),
            FsErrorCode::NotRegularFile,
        )
        .into());
    }
    let content = env.fs.read_text(&target, Some(exec.signal())).await?;
    env.ctx.emit(&FsObserved {
        target: target.clone(),
        observation: FsObservation::Present {
            version: info.version,
        },
        actor: Some(exec.clone() as FsActor),
    });
    format_file_view(
        &target.display_path,
        &content,
        env.max_output_chars,
        view_range,
    )
}

fn already_exists(target: &FsTarget, cause: Option<BoxError>) -> BoxError {
    let error = FsError::new(
        format!(
            "File already exists at: {}. Cannot overwrite files using command `create`.",
            target.display_path
        ),
        FsErrorCode::AlreadyExists,
    );
    match cause {
        Some(cause) => Box::new(error.with_cause(cause)),
        None => Box::new(error),
    }
}

async fn create_file(
    env: &Env,
    path: &str,
    file_text: Option<&str>,
    exec: &Arc<ToolExecution>,
) -> Result<String, BoxError> {
    let content = required_for_command(file_text, "file_text", "create", true)?;
    let sandbox_policy = env.policy.resolve(exec)?;
    let target = resolve_target(env, path, exec).await?;
    if env.fs.stat(&target, Some(exec.signal())).await?.is_some() {
        return Err(already_exists(&target, None));
    }
    let intent = env
        .ctx
        .waterfall(
            FsWriteIntentEvent {
                target: target.clone(),
                actor: Some(exec.clone() as FsActor),
            },
            |_| async { Ok(Some(FsWriteIntent::CreateIfAbsent)) },
        )
        .await?;
    let outcome = match env
        .fs
        .write_text(
            &target,
            content,
            intent,
            Some(exec.signal()),
            sandbox_policy.clone(),
        )
        .await
    {
        Ok(outcome) => outcome,
        Err(error) => {
            let not_observed = HarnessError::of(error.as_ref()).is_some_and(|typed| {
                typed.is_named(FsError::NAME) && typed.code() == FsErrorCode::NotObserved.as_str()
            });
            if not_observed {
                return Err(already_exists(&target, Some(error)));
            }
            return Err(MutationPolicy::map_error(error, sandbox_policy.as_ref()));
        }
    };
    env.ctx.emit(&FsObserved {
        target: target.clone(),
        observation: FsObservation::Present {
            version: outcome.version,
        },
        actor: Some(exec.clone() as FsActor),
    });
    Ok(format!(
        "New file created successfully at: {}",
        target.display_path
    ))
}

async fn replace_in_file(
    env: &Env,
    path: &str,
    old_str: Option<&str>,
    new_str: Option<&Value>,
    exec: &Arc<ToolExecution>,
) -> Result<String, BoxError> {
    if new_str.is_some_and(Value::is_null) {
        return Err(args_error(
            "Parameter `new_str` must be omitted or contain a string for command: str_replace",
        ));
    }
    let sandbox_policy = env.policy.resolve(exec)?;
    let target = resolve_target(env, path, exec).await?;
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
    let old_value = required_for_command(old_str, "old_str", "str_replace", false)?;
    let new_value = new_str.and_then(Value::as_str).unwrap_or("");
    let info = stat_existing(env, &target, Command::StrReplace, exec).await?;
    if info.r#type != FsType::File {
        return Err(FsError::new(
            format!(
                "cannot edit \"{}\": not a regular file",
                target.display_path
            ),
            FsErrorCode::NotRegularFile,
        )
        .into());
    }
    let before = env.fs.read_text(&target, Some(exec.signal())).await?;
    let offsets = match_offsets(&before, old_value);
    let Some(&offset) = offsets.first() else {
        return Err(FsError::new(
            format!(
                "No replacement was performed, old_str `{old_value}` did not appear verbatim in {}.",
                target.display_path
            ),
            FsErrorCode::EditNotFound,
        )
        .into());
    };
    if offsets.len() > 1 {
        let lines = line_numbers_at(&before, &offsets)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(FsError::new(
            format!(
                "No replacement was performed. Multiple occurrences of old_str `{old_value}` in lines [{lines}]. Please ensure it is unique"
            ),
            FsErrorCode::AmbiguousEdit,
        )
        .into());
    }
    let after = format!(
        "{}{new_value}{}",
        &before[..offset],
        &before[offset + old_value.len()..]
    );
    let expected = Some(FsWriteIntent::ReplaceIfVersion {
        version: intent.map_or(info.version, |guard| guard.version),
    });
    let outcome = env
        .fs
        .write_text(
            &target,
            &after,
            expected,
            Some(exec.signal()),
            sandbox_policy.clone(),
        )
        .await
        .map_err(|error| MutationPolicy::map_error(error, sandbox_policy.as_ref()))?;
    env.ctx.emit(&FsObserved {
        target: target.clone(),
        observation: FsObservation::Present {
            version: outcome.version,
        },
        actor: Some(exec.clone() as FsActor),
    });
    Ok(format!(
        "The file {} has been edited successfully.",
        target.display_path
    ))
}

async fn insert_in_file(
    env: &Env,
    path: &str,
    insert_line: Option<f64>,
    new_str: Option<&str>,
    exec: &Arc<ToolExecution>,
) -> Result<String, BoxError> {
    let Some(insert_line) = insert_line else {
        return Err(args_error(
            "Parameter `insert_line` is required for command: insert",
        ));
    };
    let value = required_for_command(new_str, "new_str", "insert", true)?;
    let sandbox_policy = env.policy.resolve(exec)?;
    let target = resolve_target(env, path, exec).await?;
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
    let info = stat_existing(env, &target, Command::Insert, exec).await?;
    if info.r#type != FsType::File {
        return Err(FsError::new(
            format!(
                "cannot insert into \"{}\": not a regular file",
                target.display_path
            ),
            FsErrorCode::NotRegularFile,
        )
        .into());
    }
    let before = env.fs.read_text(&target, Some(exec.signal())).await?;
    let lines: Vec<&str> = before.split('\n').collect();
    if insert_line.fract() != 0.0 || insert_line < 0.0 || insert_line > lines.len() as f64 {
        return Err(args_error(format!(
            "Invalid `insert_line` parameter: {}. It should be within the range of lines of the file: [0, {}]",
            js_number_string(insert_line),
            lines.len()
        )));
    }
    let insert_line = insert_line as usize;
    let mut after_lines = Vec::with_capacity(lines.len() + value.split('\n').count());
    after_lines.extend_from_slice(&lines[..insert_line]);
    after_lines.extend(value.split('\n'));
    after_lines.extend_from_slice(&lines[insert_line..]);
    let after = after_lines.join("\n");
    let expected = Some(FsWriteIntent::ReplaceIfVersion {
        version: intent.map_or(info.version, |guard| guard.version),
    });
    let outcome = env
        .fs
        .write_text(
            &target,
            &after,
            expected,
            Some(exec.signal()),
            sandbox_policy.clone(),
        )
        .await
        .map_err(|error| MutationPolicy::map_error(error, sandbox_policy.as_ref()))?;
    env.ctx.emit(&FsObserved {
        target: target.clone(),
        observation: FsObservation::Present {
            version: outcome.version,
        },
        actor: Some(exec.clone() as FsActor),
    });
    Ok(format!(
        "The file {} has been edited successfully.",
        target.display_path
    ))
}

fn string_field<'a>(args: &'a Value, field: &str) -> Option<&'a str> {
    args.get(field).and_then(Value::as_str)
}

fn number_field(args: &Value, field: &str) -> Option<f64> {
    args.get(field).and_then(Value::as_f64)
}

fn present_line(args: &Value) -> Option<u64> {
    let line = (number_field(args, "insert_line")? + 1.0).max(1.0);
    (line.is_finite() && line < u64::MAX as f64).then_some(line as u64)
}

fn present_editor_call(args: &Value) -> Option<ToolCallView> {
    let command = string_field(args, "command")?;
    let path = string_field(args, "path")?.to_owned();
    let location = || FileLocation {
        path: path.clone(),
        line: None,
    };
    match command {
        "view" => Some(ToolCallView::Generic(GenericCallView {
            title: format!("view {path}"),
            kind: Some(ToolCallKind::Read),
            raw_input: None,
            content: None,
            locations: Some(vec![location()]),
        })),
        "create" => Some(ToolCallView::Diff(DiffCallView {
            title: format!("create {path}"),
            diffs: vec![FileDiff {
                path: path.clone(),
                old_text: None,
                new_text: string_field(args, "file_text").unwrap_or("").to_owned(),
            }],
            locations: Some(vec![location()]),
        })),
        "str_replace" => Some(ToolCallView::Diff(DiffCallView {
            title: format!("str_replace {path}"),
            diffs: vec![FileDiff {
                path: path.clone(),
                old_text: string_field(args, "old_str").map(str::to_owned),
                new_text: string_field(args, "new_str").unwrap_or("").to_owned(),
            }],
            locations: Some(vec![location()]),
        })),
        "insert" => Some(ToolCallView::Generic(GenericCallView {
            title: format!("insert {path}"),
            kind: Some(ToolCallKind::Edit),
            raw_input: None,
            content: None,
            locations: Some(vec![FileLocation {
                path,
                line: present_line(args),
            }]),
        })),
        _ => None,
    }
}

fn parameters() -> Value {
    json!({
        "command": {
            "type": "string",
            "required": true,
            "enum": ["view", "create", "str_replace", "insert"],
            "description": "The commands to run. Allowed options are: `view`, `create`, `str_replace`, `insert`.",
        },
        "path": {
            "type": "string",
            "required": true,
            "description": "Absolute path to file or directory, e.g. `/repo/file.py` or `/repo`.",
        },
        "file_text": {
            "oneOf": [{ "type": "string" }, { "type": "null" }],
            "description": "Required string parameter of `create` command, with the content of the file to be created. A null placeholder is treated as omitted by commands that do not use this parameter.",
        },
        "insert_line": {
            "oneOf": [{ "type": "integer" }, { "type": "null" }],
            "description": "Required integer parameter of `insert` command. The `new_str` will be inserted AFTER the line `insert_line` of `path`. A null placeholder is treated as omitted by commands that do not use this parameter.",
        },
        "new_str": {
            "oneOf": [{ "type": "string" }, { "type": "null" }],
            "description": "Optional string parameter of `str_replace` command containing the new string (if omitted, no string will be added). Required string parameter of `insert` command containing the string to insert. A null placeholder is accepted only by commands that do not use this parameter.",
        },
        "old_str": {
            "oneOf": [{ "type": "string" }, { "type": "null" }],
            "description": "Required string parameter of `str_replace` command containing the string in `path` to replace. A null placeholder is treated as omitted by commands that do not use this parameter.",
        },
        "view_range": {
            "oneOf": [
                { "type": "array", "items": { "type": "integer" } },
                { "type": "null" }
            ],
            "description": "Optional parameter of `view` command when `path` points to a file. If omitted or null, the full file is shown. If provided, the file will be shown in the indicated line number range, e.g. [11, 12] will show lines 11 and 12. Indexing at 1 to start. Setting `[start_line, -1]` shows all lines from `start_line` to the end of the file.",
        },
    })
}

fn register(env: Arc<Env>, description: String) -> Result<(), BoxError> {
    let body_env = env.clone();
    let mut options = DefineToolOptions::new(
        "str_replace_editor",
        description,
        parameters(),
        DefineToolOutput {
            schema: json!({ "type": "string" }),
            render: Arc::new(|_, value| {
                vec![ContentBlock::text(value.as_str().unwrap_or_default())]
            }),
            presentation_meta: None,
        },
        move |args, exec| {
            let env = body_env.clone();
            Box::pin(async move {
                let command = Command::parse(
                    string_field(&args, "command")
                        .expect("define_tool validates required string command"),
                );
                let path = string_field(&args, "path")
                    .expect("define_tool validates required string path");
                let result = match command {
                    Command::View => {
                        let view_range =
                            args.get("view_range")
                                .and_then(Value::as_array)
                                .map(|values| {
                                    values.iter().filter_map(Value::as_f64).collect::<Vec<_>>()
                                });
                        view_path(&env, path, view_range.as_deref(), &exec).await?
                    }
                    Command::Create => {
                        create_file(&env, path, string_field(&args, "file_text"), &exec).await?
                    }
                    Command::StrReplace => {
                        replace_in_file(
                            &env,
                            path,
                            string_field(&args, "old_str"),
                            args.get("new_str"),
                            &exec,
                        )
                        .await?
                    }
                    Command::Insert => {
                        insert_in_file(
                            &env,
                            path,
                            number_field(&args, "insert_line"),
                            string_field(&args, "new_str"),
                            &exec,
                        )
                        .await?
                    }
                };
                Ok(Value::String(result))
            })
        },
    );
    options.present_call = Some(Arc::new(present_editor_call));
    env.tools.register(
        &env.ctx,
        define_tool(options)?,
        ToolRegistrationOptions::default(),
    )?;
    Ok(())
}

/// Services required by the editor plugin (`inject = ['tools', 'fs']`).
#[derive(Inject)]
pub struct Deps {
    tools: Service<Tools>,
    fs: Service<Fs>,
}

/// The `tool-str-replace-editor` plugin.
pub struct ToolStrReplaceEditor;

impl Plugin for ToolStrReplaceEditor {
    const NAME: &'static str = "tool-str-replace-editor";
    type Config = Config;
    type Deps = Deps;

    async fn apply(ctx: Context, deps: Deps, config: Cfg<Config>) -> Result<(), BoxError> {
        let config = config.get();
        if !config.max_output_chars.is_finite()
            || config.max_output_chars.fract() != 0.0
            || config.max_output_chars <= 0.0
            || config.max_output_chars > MAX_SAFE_INTEGER
        {
            return Err(
                "tool-str-replace-editor: maxOutputChars must be a positive safe integer".into(),
            );
        }
        if js_trim(&config.description).is_empty() {
            return Err("tool-str-replace-editor: description must be non-empty".into());
        }
        let policy = MutationPolicy::new(&ctx, &*deps.fs)?;
        register(
            Arc::new(Env {
                ctx,
                fs: deps.fs,
                tools: deps.tools,
                policy,
                max_output_chars: config.max_output_chars as u64,
            }),
            config.description.clone(),
        )
    }
}
