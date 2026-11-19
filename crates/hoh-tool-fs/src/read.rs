//! Model-facing UTF-8 read. It performs one provider stat for type, routing, and observed version,
//! streams large or size-unknown files, renders a bounded window, then emits the observation.

use std::sync::Arc;

use futures_util::stream;
use hoh_error::BoxError;
use hoh_fs::{FsActor, FsObservation, FsObserved, FsTextStream};
use hoh_llm::ContentBlock;
use hoh_system_prompt::{PromptSection, PromptSectionOrderName, PromptText, SystemPrompt};
use hoh_tools::{
    DefineToolOptions, DefineToolOutput, FileLocation, GenericCallView, ReadFileLine,
    ReadResultView, ToolArgsError, ToolCallKind, ToolCallView, ToolExecution,
    ToolRegistrationOptions, ToolResult, ToolResultView, define_tool,
};
use hoh_util_values::js_number_string;
use serde_json::{Map, Value, json};

use crate::Env;
use crate::read_render::{
    FileReadOutcome, FileTextLine, ReadWindow, build_window, format_read_output, lang_from_path,
    read_meta_from_meta,
};
use crate::read_target::resolve_regular_read_target;

/// Default and maximum number of lines returned by one `read` call (the `readLimit` config).
pub const READ_LIMIT: f64 = 2000.0;

/// Default streaming threshold (the `readStreamMinSize` config): files at or above this size
/// stream; smaller files read whole into memory.
pub const STREAM_MIN_SIZE: f64 = 10.0 * 1024.0 * 1024.0;

/// Resolved read-tool caps — plugin config after defaulting. Each is a positive integer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ReadToolCaps {
    /// Default and maximum number of lines returned by one call.
    pub limit: f64,
    /// Maximum characters returned for a single line.
    pub max_line_length: f64,
    /// Maximum bytes returned for selected file lines.
    pub max_bytes: f64,
    /// Files at or above this size stream; smaller files read whole into memory.
    pub stream_min_size: f64,
}

/// Validated `read` arguments after defaulting.
#[derive(Clone, Debug, PartialEq)]
pub struct ReadInput {
    pub file_path: String,
    pub offset: f64,
    pub limit: f64,
}

fn parse_positive_integer(value: &Value, name: &str) -> Result<f64, BoxError> {
    match value.as_f64() {
        Some(number) if number.is_finite() && number.fract() == 0.0 && number >= 1.0 => Ok(number),
        _ => Err(ToolArgsError::new(vec![format!("{name} must be a positive integer")]).into()),
    }
}

/// Validate value constraints the schema DSL can't express. `max_limit` is the deployment's line cap.
///
/// `args` are the schema-validated raw tool arguments; `offset`/`limit` must be positive integers
/// when given. Returns the validated input with `offset` defaulted to 1 and `limit` to `max_limit`.
pub fn parse_read_args(args: &Value, max_limit: f64) -> Result<ReadInput, BoxError> {
    let file_path = args.get("file_path").and_then(Value::as_str).unwrap_or("");
    if hoh_util_values::js_trim(file_path).is_empty() {
        return Err(ToolArgsError::new(vec!["file_path must be a non-empty string".into()]).into());
    }
    let offset = match args.get("offset") {
        None => 1.0,
        Some(value) => parse_positive_integer(value, "offset")?,
    };
    let limit = match args.get("limit") {
        None => max_limit,
        Some(value) => parse_positive_integer(value, "limit")?,
    };
    if limit > max_limit {
        return Err(ToolArgsError::new(vec![format!(
            "limit must be less than or equal to {}",
            js_number_string(max_limit)
        )])
        .into());
    }
    Ok(ReadInput {
        file_path: file_path.to_owned(),
        offset,
        limit,
    })
}

fn integer_of(value: &Value, name: &str) -> u64 {
    value.get(name).and_then(Value::as_u64).unwrap_or(0)
}

/// Register the `read` tool and its scope-aware system-prompt guidance. `caps` are the deployment's
/// resolved read caps (plugin config after defaulting).
pub fn apply_read_tool(env: &Arc<Env>, caps: ReadToolCaps) -> Result<(), BoxError> {
    let guidance_env = env.clone();
    env.system_prompt.section(
        &env.ctx,
        PromptSection {
            name: "tool:read".into(),
            order: SystemPrompt::get_section_order(PromptSectionOrderName::ToolRead),
            text: PromptText::provider(move |assemble| {
                Ok(if guidance_env.tools.get("read", assemble.scope.as_ref(), None).is_none() {
                    String::new()
                } else {
                    "Use the read tool — not shell commands like cat, sed, head, or tail — to inspect text files. Use offset and limit to read a line range or continue a large file."
                        .to_owned()
                })
            }),
            ..PromptSection::default()
        },
    )?;

    let parameters = json!({
        "file_path": { "type": "string", "required": true, "description": "Path to read, resolved by the filesystem backend." },
        "offset": { "type": "number", "description": "1-based first line to return. Defaults to 1." },
        "limit": { "type": "number", "description": format!("Maximum number of lines to return. Defaults to {}.", js_number_string(caps.limit)) },
    });
    let output = DefineToolOutput {
        schema: json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "path": { "type": "string", "required": true, "description": "Canonical absolute path in the filesystem execution world." },
                "offset": { "type": "integer", "required": true },
                "lines": {
                    "type": "array",
                    "required": true,
                    "items": {
                        "type": "object",
                        "additionalProperties": false,
                        "properties": {
                            "number": { "type": "integer", "required": true },
                            "text": { "type": "string", "required": true },
                        },
                    },
                },
                "totalLines": { "type": "integer", "required": true },
            },
        }),
        render: Arc::new(move |args, value| {
            let limit = parse_read_args(args, caps.limit).map_or(caps.limit, |input| input.limit);
            let lines = lines_of(value);
            let offset = integer_of(value, "offset");
            let total_lines = integer_of(value, "totalLines");
            let end_line = lines
                .last()
                .map_or_else(|| offset.saturating_sub(1), |line| line.number);
            let truncated_by_bytes = (lines.len() as f64) < limit && end_line < total_lines;
            vec![ContentBlock::text(format_read_output(
                value.get("path").and_then(Value::as_str).unwrap_or(""),
                &FileReadOutcome {
                    offset,
                    lines,
                    total_lines,
                    truncated_by_bytes,
                },
            ))]
        }),
        // Project the structured window into persisted `meta` so a UI's read card survives replay:
        // the raw canonical output object is not on the wire, only the model-facing text, from
        // which the line/lang data cannot be recovered.
        presentation_meta: Some(Arc::new(|_args, value| {
            let path = value.get("path").and_then(Value::as_str).unwrap_or("");
            let mut meta = Map::new();
            meta.insert("path".into(), json!(path));
            meta.insert("offset".into(), value["offset"].clone());
            meta.insert(
                "lines".into(),
                Value::Array(
                    lines_of(value)
                        .into_iter()
                        .map(|line| json!({ "number": line.number, "text": line.text }))
                        .collect(),
                ),
            );
            meta.insert("totalLines".into(), value["totalLines"].clone());
            if let Some(lang) = lang_from_path(path) {
                meta.insert("lang".into(), json!(lang));
            }
            Value::Object(meta)
        })),
    };
    let body_env = env.clone();
    let mut options = DefineToolOptions::new(
        "read",
        "Read a UTF-8 text file and return line-numbered content.",
        parameters,
        output,
        move |args, exec| {
            let env = body_env.clone();
            Box::pin(async move { execute(&env, caps, &args, &exec).await })
        },
    );
    // Observation races fail closed because guarded mutations re-check the version in-lock.
    options.is_concurrency_safe = Some(Arc::new(|_args| true));
    // Result-time display: a `read` card carrying the structured line window a capable UI renders
    // as a line-numbered, syntax-highlighted view. The structured data is narrowed from the
    // persisted `meta` (replay-safe); the envelope-stripped model-facing text rides along as
    // `content` so a UI without the read capability still shows the file text. A malformed or
    // absent meta, or a result whose text is not the read envelope, declines to `None` (the generic
    // fallback), never failing on replay of obsolete logged output.
    options.present_result = Some(Arc::new(|_args, result: &ToolResult| {
        present_result(result)
    }));
    // Pure display: a generic card titled by the file with the read window appended (`Read
    // foo.txt (5 - 8)`), `read` kind (icon), and a follow-along location whose line is the read's
    // offset (defaulting to 1). The window reflects raw args, so an omitted limit keeps the title
    // bare instead of smuggling config into this pure presenter.
    options.present_call = Some(Arc::new(|args| Some(present_call(args))));
    env.tools.register(
        &env.ctx,
        define_tool(options)?,
        ToolRegistrationOptions::default(),
    )?;
    Ok(())
}

fn lines_of(value: &Value) -> Vec<FileTextLine> {
    value
        .get("lines")
        .and_then(Value::as_array)
        .map(|lines| {
            lines
                .iter()
                .map(|line| FileTextLine {
                    number: line.get("number").and_then(Value::as_u64).unwrap_or(0),
                    text: line
                        .get("text")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned(),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The body of a `read` envelope: `<path>…</path>\n<type>file</type>\n<content>\n BODY \n</content>`.
fn envelope_body(text: &str) -> Option<&str> {
    let (first, rest) = text.split_once('\n')?;
    first.strip_prefix("<path>")?.strip_suffix("</path>")?;
    rest.strip_prefix("<type>file</type>\n<content>\n")?
        .strip_suffix("\n</content>")
}

fn present_result(result: &ToolResult) -> Option<ToolResultView> {
    if result.is_error {
        return None;
    }
    let meta = read_meta_from_meta(result.meta.as_ref())?;
    let [only] = result.content.as_slice() else {
        return None;
    };
    if only.r#type != "text" {
        return None;
    }
    let body = envelope_body(only.text_value()?)?;
    Some(ToolResultView::Read(ReadResultView {
        title: None,
        path: meta.path,
        offset: meta.offset,
        lines: meta
            .lines
            .into_iter()
            .map(|line| ReadFileLine {
                number: line.number,
                text: line.text,
            })
            .collect(),
        total_lines: meta.total_lines,
        lang: meta.lang,
        content: Some(vec![ContentBlock::text(body)]),
    }))
}

fn present_call(args: &Value) -> ToolCallView {
    let offset = args.get("offset").and_then(Value::as_f64);
    let limit = args.get("limit").and_then(Value::as_f64);
    let start = offset.unwrap_or(1.0);
    let window = match (limit, offset) {
        (Some(limit), _) if limit > 0.0 => {
            format!(
                " ({} - {})",
                js_number_string(start),
                js_number_string(start + limit - 1.0)
            )
        }
        (_, Some(offset)) => format!(" (from line {})", js_number_string(offset)),
        _ => String::new(),
    };
    let path = args.get("file_path").and_then(Value::as_str).unwrap_or("");
    ToolCallView::Generic(GenericCallView {
        title: format!("Read {path}{window}"),
        kind: Some(ToolCallKind::Read),
        raw_input: None,
        content: None,
        // Whole-number lines only: a fractional offset is refused at execution.
        locations: Some(vec![FileLocation {
            path: path.to_owned(),
            line: Some(start as u64),
        }]),
    })
}

async fn execute(
    env: &Env,
    caps: ReadToolCaps,
    args: &Value,
    exec: &Arc<ToolExecution>,
) -> Result<Value, BoxError> {
    let input = parse_read_args(args, caps.limit)?;
    // One stat: absence observation OR type check + size routing + present version. A concurrent
    // write can only make a later guarded mutation fail stale and require reread.
    let (target, info) = resolve_regular_read_target(env, exec, &input.file_path).await?;

    // Stream when the file is large OR size is unknown, so a size-less backend never buffers an
    // arbitrarily large file.
    let chunks: FsTextStream = match info.size {
        Some(size) if (size as f64) < caps.stream_min_size => {
            let text = env.fs.read_text(&target, Some(exec.signal())).await?;
            Box::pin(stream::iter([Ok(text)]))
        }
        _ => env.fs.stream_text(&target, Some(exec.signal())).await?,
    };
    let window = build_window(
        chunks,
        &ReadWindow {
            offset: input.offset,
            limit: input.limit as u64,
            max_line_length: caps.max_line_length as u64,
            max_bytes: caps.max_bytes as u64,
        },
        &target.display_path,
    )
    .await?;

    let outcome = json!({
        "path": env.fs.process_path(&target),
        "offset": input.offset as u64,
        "lines": window.lines.iter().map(|line| json!({ "number": line.number, "text": line.text })).collect::<Vec<_>>(),
        "totalLines": window.total_lines,
    });
    // Record the present observation (a no-op when no policy plugin listens). The read already
    // succeeded; an fs/observed listener is contractually a synchronous, side-effect-only recorder.
    env.ctx.emit(&FsObserved {
        target,
        observation: FsObservation::Present {
            version: info.version,
        },
        actor: Some(exec.clone() as FsActor),
    });
    Ok(outcome)
}
