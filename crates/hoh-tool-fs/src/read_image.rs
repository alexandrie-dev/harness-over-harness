//! The model-facing `read_image` tool commits a PNG/JPEG/WebP/GIF file. A path without a file
//! extension is identified from its file signature, while the attachment service's full decode
//! stays authoritative. The mounted `fs` backend owns path resolution and read access; names only
//! declare media type.
//!
//! The route gate is deliberately stricter than the host upload preflight. An image-reading tool is
//! useful only when the exact calling route can inspect its result, so unknown capability refuses
//! instead of relying on an adapter failure after filesystem and attachment work.

use std::sync::Arc;

use hoh_attachment::{
    AttachmentError, AttachmentErrorCode, AttachmentId, AttachmentStore, Attachments,
    ImageAttachmentRef, ImageMediaType, ProjectedDimensions, SaveImageAttachment,
};
use hoh_error::{BoxError, HarnessError};
use hoh_fs::{FsActor, FsObservation, FsObserved};
use hoh_llm::{ContentBlock, ImageBlock, Llm};
use hoh_tools::{
    DefineToolOptions, DefineToolOutput, FileLocation, GenericCallView, ToolArgsError,
    ToolCallKind, ToolCallView, ToolContextUnavailableError, ToolExecution,
    ToolRegistrationOptions, define_tool,
};
use serde_json::{Map, Value, json};

use crate::Env;
use crate::image_error::{ImageInputError, ImageInputErrorCode};
use crate::read_target::resolve_regular_read_target;
use crate::shim::live_agent;
use hoh_util_workspace_path::node_path;

/// Extensions `read_image` accepts; magic-byte validation at the attachment service stays
/// authoritative.
fn media_type_for_extension(extension: &str) -> Option<ImageMediaType> {
    match extension {
        ".png" => Some(ImageMediaType::Png),
        ".jpg" | ".jpeg" => Some(ImageMediaType::Jpeg),
        ".webp" => Some(ImageMediaType::Webp),
        ".gif" => Some(ImageMediaType::Gif),
        _ => None,
    }
}

const PNG_SIGNATURE: [u8; 8] = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];
const JPEG_SIGNATURE: [u8; 3] = [0xff, 0xd8, 0xff];

fn matches_bytes(data: &[u8], offset: usize, expected: &[u8]) -> bool {
    data.get(offset..offset + expected.len())
        .is_some_and(|window| window == expected)
}

/// Identify the media type declared by a supported image file signature.
///
/// `data` is the file bytes read through the current filesystem backend. Returns the detected
/// supported media type, or `None` for other bytes.
pub fn sniff_image_media_type(data: &[u8]) -> Option<ImageMediaType> {
    if matches_bytes(data, 0, &PNG_SIGNATURE) {
        Some(ImageMediaType::Png)
    } else if matches_bytes(data, 0, &JPEG_SIGNATURE) {
        Some(ImageMediaType::Jpeg)
    } else if matches_bytes(data, 0, b"GIF87a") || matches_bytes(data, 0, b"GIF89a") {
        Some(ImageMediaType::Gif)
    } else if matches_bytes(data, 0, b"RIFF") && matches_bytes(data, 8, b"WEBP") {
        Some(ImageMediaType::Webp)
    } else {
        None
    }
}

/// Map a model-supplied path to its declared image media type by extension.
///
/// `file_path` is the raw `file_path` argument (not yet resolved). Returns the declared media type,
/// or `None` when the path does not claim an image.
pub fn image_media_type_for_path(file_path: &str) -> Option<ImageMediaType> {
    media_type_for_extension(&node_path::extname(file_path).to_lowercase())
}

fn image_value_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": true,
        "properties": {
            "attachmentId": { "type": "string", "required": true },
            "mediaType": { "type": "string", "enum": ["image/png", "image/jpeg", "image/webp", "image/gif"], "required": true },
            "bytes": { "type": "integer", "required": true },
            "width": { "type": "integer", "required": true },
            "height": { "type": "integer", "required": true },
            "name": { "type": "string" },
            "originalDimensions": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "width": { "type": "integer", "required": true },
                    "height": { "type": "integer", "required": true },
                },
            },
        },
    })
}

/// Enforce the strict image-capability gate for the calling route. Resolves the session's latest
/// routed provider/model (request header config, then agent options) and requires the exact
/// resolved route to declare `image` input explicitly.
///
/// `requested_path` is the raw, not-yet-resolved path rendered in refusal messages. Fails with
/// `ToolContextUnavailableError` when no Agent route or `llm` service is available, and with
/// `ImageInputError` `MODEL_DOES_NOT_SUPPORT_IMAGES` when the route does not declare image input.
pub async fn assert_image_capable_route(
    env: &Env,
    exec: &Arc<ToolExecution>,
    requested_path: &str,
) -> Result<(), BoxError> {
    let header = exec
        .agent
        .as_ref()
        .and_then(|agent| agent.session().request_header());
    let routed = header.as_deref().and_then(|header| header.get("config"));
    let routed_text = |key: &str| {
        routed
            .and_then(|config| config.get(key))
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    let live = exec
        .agent
        .as_ref()
        .and_then(|agent| live_agent(&env.ctx, agent));
    let agent_provider = live.as_ref().and_then(|agent| agent.options().provider());
    let agent_model = live.as_ref().and_then(|agent| agent.options().model());
    let provider = routed_text("provider").or(agent_provider);
    let model = routed_text("model").or(agent_model);
    let llm = env.ctx.get::<Llm>();
    let (Some(provider), Some(model), Some(llm)) = (provider, model, llm) else {
        return Err(ToolContextUnavailableError::new(format!(
            "cannot read \"{requested_path}\" as an image: the current model route could not be resolved"
        ))
        .into());
    };
    let active = llm
        .resolve_model_info(&provider, &model, Some(&exec.signal()))
        .await?;
    if !active
        .model
        .input_modalities
        .as_ref()
        .is_some_and(|modalities| modalities.iter().any(|modality| modality == "image"))
    {
        return Err(ImageInputError::new(
            format!(
                "cannot read \"{requested_path}\" as an image: model \"{model}\" does not declare image input; switch to an image-capable model to read images"
            ),
            ImageInputErrorCode::ModelDoesNotSupportImages,
        )
        .into());
    }
    Ok(())
}

/// Refuse a media type outside the deployment's accepted set, naming the offending path.
fn assert_deployment_accepts(
    attachments: &dyn AttachmentStore,
    media_type: ImageMediaType,
    display_path: &str,
) -> Result<(), BoxError> {
    if !attachments.image_limits().media_types.contains(&media_type) {
        return Err(ImageInputError::new(
            format!(
                "cannot read \"{display_path}\": {} images are not accepted by this deployment",
                media_type.as_str()
            ),
            ImageInputErrorCode::UnsupportedFormat,
        )
        .into());
    }
    Ok(())
}

/// Re-brand a structured image outcome into the durable attachment reference an `ImageBlock`
/// carries. `image` is the image metadata from the output schema; `None` when it is malformed.
pub fn image_ref_from_value(image: &Value) -> Option<ImageAttachmentRef> {
    let media_type = serde_json::from_value(image.get("mediaType")?.clone()).ok()?;
    let original_dimensions = match image.get("originalDimensions") {
        None | Some(Value::Null) => None,
        Some(dimensions) => Some(ProjectedDimensions {
            width: dimensions.get("width")?.as_u64()?,
            height: dimensions.get("height")?.as_u64()?,
        }),
    };
    Some(ImageAttachmentRef {
        attachment_id: AttachmentId::from(image.get("attachmentId")?.as_str()?),
        media_type,
        bytes: image.get("bytes")?.as_u64()?,
        width: image.get("width")?.as_u64()?,
        height: image.get("height")?.as_u64()?,
        name: image.get("name").and_then(Value::as_str).map(str::to_owned),
        original_dimensions,
    })
}

/// JS `Number.prototype.toFixed(2)` for a positive ratio. `toFixed` rounds an exact tie up;
/// `format!` rounds it to even (9x9 → 8x8 is 1.125: JS says 1.13, `{:.2}` says 1.12).
fn fixed2(value: f64) -> String {
    let exact = format!("{value:.40}");
    let Some((whole, fraction)) = exact.split_once('.') else {
        return format!("{value:.2}");
    };
    let tie = fraction.as_bytes()[2] == b'5' && fraction[3..].bytes().all(|b| b == b'0');
    match (tie, whole.parse::<u64>(), fraction[..2].parse::<u64>()) {
        (true, Ok(whole), Ok(cents)) => {
            let total = whole * 100 + cents + 1;
            format!("{}.{:02}", total / 100, total % 100)
        }
        _ => format!("{value:.2}"),
    }
}

/// Format an image read as the model-facing envelope beside its image block. A downscaled read
/// names the on-disk dimensions and the multiplier that maps coordinates measured on the attached
/// image back onto the original file.
///
/// `display_path` is the backend-resolved path rendered in the envelope's `<path>` element and
/// `image` the image metadata to summarize. The image itself rides the adjacent image block.
pub fn format_image_read_output(display_path: &str, image: &ImageAttachmentRef) -> String {
    let mut scaled = String::new();
    if let Some(original) = &image.original_dimensions {
        // Integer rounding can give the two axes slightly different ratios, so the advice names
        // one multiplier only when both round to the same value.
        let x = fixed2(original.width as f64 / image.width as f64);
        let y = fixed2(original.height as f64 / image.height as f64);
        let advice = if x == y {
            format!("multiply coordinates by {x}")
        } else {
            format!("multiply x coordinates by {x} and y coordinates by {y}")
        };
        scaled = format!(
            " (downscaled from {}x{} px; {advice} to locate features in the original file)",
            original.width, original.height
        );
    }
    format!(
        "<path>{display_path}</path>\n<type>image</type>\n<content>\n{} image, {}x{} px, {} bytes{scaled}\n</content>",
        image.media_type.as_str(),
        image.width,
        image.height,
        image.bytes
    )
}

/// Facts about one image read that a save-failure report names.
struct SaveFailureFacts<'a> {
    display_path: &'a str,
    /// Lower-cased path extension; empty for an extension-less path.
    extension: &'a str,
    media_type: ImageMediaType,
    /// Whether `media_type` came from the file signature instead of the extension.
    sniffed: bool,
}

/// Add the read path and the next step to one `save_image` rejection. The attachment store
/// classifies its own failures, so an `AttachmentError` keeps its `name` and `code` and only gains
/// a message naming the file and remedy, with the original as `cause`; a code without a remedy, and
/// any value that is not an `AttachmentError`, passes through unchanged.
fn save_failure(
    error: BoxError,
    attachments: &dyn AttachmentStore,
    facts: &SaveFailureFacts,
) -> BoxError {
    let Some(found) =
        HarnessError::of(error.as_ref()).filter(|found| found.is_named(AttachmentError::NAME))
    else {
        return error;
    };
    let Some(code) = AttachmentErrorCode::from_code(found.code()) else {
        return error;
    };
    match save_failure_detail(code, found.message(), attachments, facts) {
        None => error,
        Some(detail) => Box::new(
            AttachmentError::new(
                format!("cannot read \"{}\": {detail}", facts.display_path),
                code,
            )
            .with_cause(found.clone()),
        ),
    }
}

/// The model-facing remedy for one attachment rejection, or `None` when the store message stands.
fn save_failure_detail(
    code: AttachmentErrorCode,
    message: &str,
    attachments: &dyn AttachmentStore,
    facts: &SaveFailureFacts,
) -> Option<String> {
    let SaveFailureFacts {
        extension,
        media_type,
        sniffed,
        ..
    } = facts;
    let media = media_type.as_str();
    let limits = attachments.image_limits();
    // Size refusals stay recoverable tool errors: an oversized image must never enter durable
    // history, where it would ride every later model request past provider-side dimension
    // rejections.
    match code {
        AttachmentErrorCode::ImageDimensionTooLarge => Some(format!(
            "at least one image side exceeds the {}px limit; downscale the image and read the smaller copy",
            limits.max_image_dimension
        )),
        AttachmentErrorCode::ImageTooManyPixels => Some(format!(
            "the image exceeds the {}-pixel decoded-size limit; downscale the image and read the smaller copy",
            limits.max_image_pixels
        )),
        AttachmentErrorCode::ImageTooLarge | AttachmentErrorCode::ImagesTooLarge => Some(
            "the image cannot be stored within the deployment's byte limits; downscale the image and read the smaller copy"
                .to_owned(),
        ),
        AttachmentErrorCode::UnsupportedImageType => {
            Some(format!("{media} images are not accepted by this deployment"))
        }
        AttachmentErrorCode::InvalidImage => Some(
            "the bytes do not decode as a supported PNG/JPEG/WebP/GIF image; the file may be truncated or corrupt"
                .to_owned(),
        ),
        AttachmentErrorCode::ImageTypeMismatch => Some(if *sniffed {
            format!("the file signature claims {media}, but the bytes decode as a different image format; the file may be corrupt")
        } else {
            format!("the {extension} extension declares {media}, but the bytes use a different image format; rename the file to match its actual format if it is PNG/JPEG/WebP/GIF, or convert it to one of those formats")
        }),
        AttachmentErrorCode::AttachmentWriteFailed => {
            message.to_lowercase().contains("16-bit png").then(|| {
                "the 16-bit PNG could not be converted to the normalized 8-bit sRGB form; convert it to an 8-bit PNG/JPEG/WebP and retry".to_owned()
            })
        }
        AttachmentErrorCode::TooManyImages
        | AttachmentErrorCode::InvalidImageBase64
        | AttachmentErrorCode::InvalidFileBase64
        | AttachmentErrorCode::InvalidAttachmentRef
        | AttachmentErrorCode::AttachmentCorrupt
        | AttachmentErrorCode::AttachmentNotFound
        | AttachmentErrorCode::AttachmentReadFailed
        | AttachmentErrorCode::AttachmentProjectionUnsupported
        | AttachmentErrorCode::AttachmentFilesUnsupported => None,
    }
}

fn image_value(path: String, reference: &ImageAttachmentRef) -> Value {
    let mut image = Map::new();
    image.insert("attachmentId".into(), json!(reference.attachment_id));
    image.insert("mediaType".into(), json!(reference.media_type));
    image.insert("bytes".into(), json!(reference.bytes));
    image.insert("width".into(), json!(reference.width));
    image.insert("height".into(), json!(reference.height));
    if let Some(name) = &reference.name {
        image.insert("name".into(), json!(name));
    }
    if let Some(original) = &reference.original_dimensions {
        image.insert(
            "originalDimensions".into(),
            json!({ "width": original.width, "height": original.height }),
        );
    }
    json!({ "path": path, "image": Value::Object(image) })
}

/// Project one structured image read into its model-facing envelope and image: the two content
/// blocks used by native and nested dispatches.
fn image_read_content(value: &Value) -> Vec<ContentBlock> {
    let path = value.get("path").and_then(Value::as_str).unwrap_or("");
    match value.get("image").and_then(image_ref_from_value) {
        Some(reference) => vec![
            ContentBlock::text(format_image_read_output(path, &reference)),
            ImageBlock {
                attachment: reference,
                offloaded: None,
            }
            .into(),
        ],
        None => Vec::new(),
    }
}

/// Register the `read_image` tool. The composing plugin owns the attachments gate: `lib.rs` calls
/// this inside the `attachments` injection so the tool exists only while a durable store is
/// mounted. Execution still re-checks the service for direct callers and gates on the calling
/// route's declared image input.
pub fn apply_read_image_tool(env: &Arc<Env>) -> Result<(), BoxError> {
    let parameters = json!({
        "file_path": { "type": "string", "required": true, "description": "Path to the image file, resolved by the filesystem backend." },
    });
    let output = DefineToolOutput {
        schema: json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "path": { "type": "string", "required": true, "description": "Canonical absolute path in the filesystem execution world." },
                "image": image_value_schema(),
            },
        }),
        render: Arc::new(|_args, value| image_read_content(value)),
        // Persist the resolved path only. The attachment reference is NOT copied here: the settled
        // `content` already carries the image block with the complete reference, so a second copy
        // would keep two records of one fact — and a `tools/post-execute` hook that legitimately
        // replaces the content would leave the stale copy behind, showing an image the result no
        // longer returns. The path needs its own structured record because the content carries it
        // only as model-facing envelope text, which the client does not parse.
        presentation_meta: Some(Arc::new(|_args, value| json!({ "path": value["path"] }))),
    };
    let body_env = env.clone();
    let mut options = DefineToolOptions::new(
        "read_image",
        concat!(
            "Read a PNG/JPEG/WebP/GIF file and return the image itself. ",
            "Large images are downscaled automatically; do not install image libraries or create thumbnails to inspect an image.",
        ),
        parameters,
        output,
        move |args, exec| {
            let env = body_env.clone();
            Box::pin(async move { execute(&env, &args, &exec).await })
        },
    );
    // Content-addressed attachment writes are idempotent, so concurrent reads of the same file
    // cannot conflict.
    options.is_concurrency_safe = Some(Arc::new(|_args| true));
    // Pure display: a generic card in the read family with a follow-along location on the image
    // file.
    options.present_call = Some(Arc::new(|args| {
        let path = args.get("file_path").and_then(Value::as_str).unwrap_or("");
        Some(ToolCallView::Generic(GenericCallView {
            title: format!("Read image {path}"),
            kind: Some(ToolCallKind::Read),
            raw_input: None,
            content: None,
            locations: Some(vec![FileLocation {
                path: path.to_owned(),
                line: None,
            }]),
        }))
    }));
    env.tools.register(
        &env.ctx,
        define_tool(options)?,
        ToolRegistrationOptions::default(),
    )?;
    Ok(())
}

async fn execute(env: &Env, args: &Value, exec: &Arc<ToolExecution>) -> Result<Value, BoxError> {
    let file_path = args.get("file_path").and_then(Value::as_str).unwrap_or("");
    if hoh_util_values::js_trim(file_path).is_empty() {
        return Err(ToolArgsError::new(vec!["file_path must be a non-empty string".into()]).into());
    }

    // Every pre-read gate runs before any filesystem I/O so a refusal never leaks partial reads or
    // attachment writes. An extension-less path declares no format, so only its format and
    // deployment media-type checks wait for the bytes.
    let extension = node_path::extname(file_path).to_lowercase();
    let declared = media_type_for_extension(&extension);
    if declared.is_none() && !extension.is_empty() {
        return Err(ImageInputError::new(
            format!(
                "cannot read \"{file_path}\": the {extension} extension does not declare a supported image format; read_image accepts PNG/JPEG/WebP/GIF files, including extension-less files in those formats"
            ),
            ImageInputErrorCode::UnsupportedFormat,
        )
        .into());
    }
    let Some(attachments) = env.ctx.get::<Attachments>() else {
        return Err(ToolContextUnavailableError::new(format!(
            "cannot read \"{file_path}\" as an image: no attachment service is mounted"
        ))
        .into());
    };
    if let Some(declared) = declared {
        assert_deployment_accepts(&*attachments, declared, file_path)?;
    }
    assert_image_capable_route(env, exec, file_path).await?;

    let (target, info) = resolve_regular_read_target(env, exec, file_path).await?;

    // The tool result is one message carrying one image, so the per-message aggregate bound
    // applies beside the per-image bound.
    let limits = attachments.image_limits();
    let byte_cap = limits.max_image_bytes.min(limits.max_message_image_bytes);
    let data = env
        .fs
        .read_bytes(&target, Some(exec.signal()), byte_cap)
        .await?;
    let Some(media_type) = declared.or_else(|| sniff_image_media_type(&data)) else {
        return Err(ImageInputError::new(
            format!(
                "cannot read \"{}\": the file content is not a supported image format; read_image accepts PNG/JPEG/WebP/GIF",
                target.display_path
            ),
            ImageInputErrorCode::UnsupportedFormat,
        )
        .into());
    };
    if declared.is_none() {
        assert_deployment_accepts(&*attachments, media_type, &target.display_path)?;
    }
    // Persist before returning: the image block must reference a durably committed object by the
    // time the tool/result event is appended.
    let input = SaveImageAttachment {
        data,
        media_type,
        name: Some(node_path::basename(&target.display_path)),
    };
    let reference = match attachments.save_image(&input).await {
        Ok(reference) => reference,
        Err(error) => {
            return Err(save_failure(
                error,
                &*attachments,
                &SaveFailureFacts {
                    display_path: &target.display_path,
                    extension: &extension,
                    media_type,
                    sniffed: declared.is_none(),
                },
            ));
        }
    };
    env.ctx.emit(&FsObserved {
        target: target.clone(),
        observation: FsObservation::Present {
            version: info.version,
        },
        actor: Some(exec.clone() as FsActor),
    });
    Ok(image_value(env.fs.process_path(&target), &reference))
}
