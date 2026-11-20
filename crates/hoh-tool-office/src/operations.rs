use std::sync::Arc;

use hoh_agent::Agent as LiveAgent;
use hoh_attachment::{AttachmentError, AttachmentErrorCode, ImageMediaType, SaveImageAttachment};
use hoh_error::{BoxError, HarnessError, SharedError};
use hoh_fs::{FsByteRange, FsInfo, FsResolveOptions, FsTarget, FsType};
use hoh_office_kit::{
    OfficeConversionRequest, OfficeDocumentExtension, OfficeFileResult, OfficeImageInputExtension,
    OfficeImagesRequest, OfficeKit, OfficeKitError, OfficeKitErrorCode, OfficeKitSource,
    OfficeOutputTarget, OfficeRecalculationRequest, OfficeSourceKey, OfficeSourceRead,
    OfficeWorkbookExtension,
};
use hoh_timeout::AbortSignal;
use hoh_tools::{ToolContextUnavailableError, ToolExecution};

use crate::{
    Config, Env,
    content::{OfficeFileValue, OfficeRenderValue, OfficeRenderedImage},
    error::{OfficeToolError, OfficeToolErrorCode},
    formats::{
        OfficeConvertArgs, OfficeOperationName, OfficeRecalculateArgs, OfficeRenderArgs,
        extension_of,
    },
    shim::Subagents,
};

struct Prepared {
    agent: Arc<dyn LiveAgent>,
    kit: hoh_kernel::Service<OfficeKit>,
    target: FsTarget,
    source: OfficeKitSource,
}

fn box_error(error: impl std::error::Error + Send + Sync + 'static) -> BoxError {
    Box::new(error)
}

fn office_failure(verb: &str, path: &str, error: SharedError) -> BoxError {
    let Some(kit_error) = error.downcast_ref::<OfficeKitError>() else {
        return Box::new(error);
    };
    let code = kit_error.code;
    let detail = kit_error.message.clone();
    let hint = match code {
        OfficeKitErrorCode::Unavailable => {
            " Tell the user this operation is unavailable, or retry later if the cause may be temporary; do not substitute system soffice, npx, or another converter."
        }
        OfficeKitErrorCode::Busy => " Retry after other document operations finish.",
        OfficeKitErrorCode::OutputTooLarge if verb == "render" => {
            " Select fewer pages, a smaller range, or a lower dpi."
        }
        _ => "",
    };
    Box::new(OfficeToolError::with_cause(
        format!("cannot {verb} \"{path}\": {detail}{hint}"),
        code.into(),
        box_error(error),
    ))
}

fn classify_box(verb: &str, path: &str, error: BoxError) -> BoxError {
    if error.downcast_ref::<OfficeKitError>().is_some() {
        office_failure(verb, path, SharedError::new(error))
    } else {
        error
    }
}

async fn assert_unchanged(
    env: &Env,
    target: &FsTarget,
    info: &FsInfo,
    signal: &AbortSignal,
) -> Result<(), BoxError> {
    let current = env.fs.stat(target, Some(signal.clone())).await?;
    if !current
        .is_some_and(|current| current.r#type == FsType::File && current.version == info.version)
    {
        return Err(box_error(OfficeKitError::new(
            OfficeKitErrorCode::SourceChanged,
            "The source changed.",
        )));
    }
    Ok(())
}

async fn authorize_content(
    env: &Env,
    target: &FsTarget,
    info: &FsInfo,
    signal: &AbortSignal,
) -> Result<(), BoxError> {
    env.fs
        .read_byte_range(
            target,
            FsByteRange {
                offset: 0,
                length: 1,
            },
            Some(signal.clone()),
        )
        .await?;
    assert_unchanged(env, target, info, signal).await
}

fn office_source(env: &Env, session_id: String, target: FsTarget, info: FsInfo) -> OfficeKitSource {
    let process_path = env.fs.process_path(&target);
    let key = serde_json::to_string(&[session_id, process_path])
        .expect("Office source key is JSON serializable");
    let version = info.version.to_string();
    let bytes = info.size;
    let fs = env.fs.clone();
    OfficeKitSource {
        key: OfficeSourceKey::from(key),
        version: version.clone(),
        bytes,
        read: Arc::new(move |signal, max_bytes| {
            let fs = fs.clone();
            let target = target.clone();
            let info = info.clone();
            let version = version.clone();
            Box::pin(async move {
                let current = fs.stat(&target, Some(signal.clone())).await?;
                if !current.is_some_and(|current| {
                    current.r#type == FsType::File && current.version == info.version
                }) {
                    return Err(box_error(OfficeKitError::new(
                        OfficeKitErrorCode::SourceChanged,
                        "The source changed.",
                    )));
                }
                let data = fs
                    .read_bytes(&target, Some(signal.clone()), max_bytes)
                    .await
                    .map_err(|error| {
                        if HarnessError::of(error.as_ref())
                            .is_some_and(|error| error.code() == "FS_TOO_LARGE")
                        {
                            box_error(OfficeKitError::with_source(
                                OfficeKitErrorCode::InputTooLarge,
                                "The source exceeds the reserved byte capacity.",
                                error,
                            ))
                        } else {
                            error
                        }
                    })?;
                let current = fs.stat(&target, Some(signal)).await?;
                if !current.is_some_and(|current| {
                    current.r#type == FsType::File && current.version == info.version
                }) {
                    return Err(box_error(OfficeKitError::new(
                        OfficeKitErrorCode::SourceChanged,
                        "The source changed.",
                    )));
                }
                Ok(OfficeSourceRead {
                    bytes: data,
                    version,
                })
            })
        }),
    }
}

async fn prepare_input(
    env: &Env,
    exec: &ToolExecution,
    operation: OfficeOperationName,
    input: &str,
) -> Result<Prepared, BoxError> {
    let verb = operation.as_str();
    let Some(calling) = &exec.agent else {
        return Err(box_error(ToolContextUnavailableError::new(format!(
            "cannot {verb} \"{input}\": Office tools run only in an Agent step"
        ))));
    };
    let kit = env.ctx.get::<OfficeKit>().ok_or_else(|| {
        office_failure(
            verb,
            input,
            SharedError::new(box_error(OfficeKitError::new(
                OfficeKitErrorCode::Unavailable,
                "The document converter is unavailable.",
            ))),
        )
    })?;
    let session = calling.session();
    let agent = env.agents.get(session.id()).ok_or_else(|| {
        box_error(ToolContextUnavailableError::new(format!(
            "cannot {verb} \"{input}\": the calling Agent is no longer registered"
        )))
    })?;
    let signal = exec.signal();
    let cwd = env.working_directory.ensure(&agent, Some(&signal)).await?;
    let target = env
        .fs
        .resolve(
            input,
            FsResolveOptions {
                cwd: Some(cwd),
                signal: Some(signal.clone()),
            },
        )
        .await?;
    let info = env.fs.stat(&target, Some(signal.clone())).await?;
    let Some(info) = info else {
        return Err(box_error(hoh_fs::FsError::new(
            format!("cannot read \"{}\": not found", target.display_path),
            hoh_fs::FsErrorCode::NotFound,
        )));
    };
    if info.r#type != FsType::File {
        return Err(box_error(hoh_fs::FsError::new(
            format!(
                "cannot read \"{}\": not a regular file",
                target.display_path
            ),
            hoh_fs::FsErrorCode::NotRegularFile,
        )));
    }
    authorize_content(env, &target, &info, &signal)
        .await
        .map_err(|error| classify_box(verb, &target.display_path, error))?;
    let source = office_source(env, session.id().to_string(), target.clone(), info);
    Ok(Prepared {
        agent,
        kit,
        target,
        source,
    })
}

fn image_extension(path: &str) -> OfficeImageInputExtension {
    match extension_of(path).as_str() {
        "doc" => OfficeImageInputExtension::Doc,
        "docx" => OfficeImageInputExtension::Docx,
        "xls" => OfficeImageInputExtension::Xls,
        "xlsx" => OfficeImageInputExtension::Xlsx,
        "ppt" => OfficeImageInputExtension::Ppt,
        "pptx" => OfficeImageInputExtension::Pptx,
        "odt" => OfficeImageInputExtension::Odt,
        "ods" => OfficeImageInputExtension::Ods,
        "odp" => OfficeImageInputExtension::Odp,
        "pdf" => OfficeImageInputExtension::Pdf,
        _ => unreachable!("input family was validated"),
    }
}

fn document_extension(path: &str) -> OfficeDocumentExtension {
    match extension_of(path).as_str() {
        "doc" => OfficeDocumentExtension::Doc,
        "docx" => OfficeDocumentExtension::Docx,
        "xls" => OfficeDocumentExtension::Xls,
        "xlsx" => OfficeDocumentExtension::Xlsx,
        "ppt" => OfficeDocumentExtension::Ppt,
        "pptx" => OfficeDocumentExtension::Pptx,
        "odt" => OfficeDocumentExtension::Odt,
        "ods" => OfficeDocumentExtension::Ods,
        "odp" => OfficeDocumentExtension::Odp,
        _ => unreachable!("input family was validated"),
    }
}

fn workbook_extension(path: &str) -> OfficeWorkbookExtension {
    match extension_of(path).as_str() {
        "xls" => OfficeWorkbookExtension::Xls,
        "xlsx" => OfficeWorkbookExtension::Xlsx,
        "ods" => OfficeWorkbookExtension::Ods,
        _ => unreachable!("input family was validated"),
    }
}

fn route(agent: &dyn LiveAgent) -> (Option<String>, Option<String>) {
    let header = agent.session().request_header();
    let config = header.as_ref().and_then(|header| header.get("config"));
    let provider = config
        .and_then(|config| config.get("provider"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .or_else(|| agent.options().provider());
    let model = config
        .and_then(|config| config.get("model"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .or_else(|| agent.options().model());
    (provider, model)
}

async fn assert_render_route(
    env: &Env,
    agent: &dyn LiveAgent,
    signal: &AbortSignal,
    path: &str,
) -> Result<(), BoxError> {
    let Some(llm) = env.ctx.get::<hoh_llm::Llm>() else {
        return Err(box_error(ToolContextUnavailableError::new(format!(
            "cannot render \"{path}\": the current model route could not be resolved"
        ))));
    };
    let (Some(provider), Some(model)) = route(agent) else {
        return Err(box_error(ToolContextUnavailableError::new(format!(
            "cannot render \"{path}\": the current model route could not be resolved"
        ))));
    };
    let info = llm
        .resolve_model_info(&provider, &model, Some(signal))
        .await?;
    if !info
        .model
        .input_modalities
        .as_deref()
        .unwrap_or_default()
        .iter()
        .any(|modality| modality == "image")
    {
        return Err(box_error(OfficeToolError::new(
            format!("cannot render \"{path}\": model \"{model}\" does not accept images"),
            OfficeToolErrorCode::ModelDoesNotSupportImages,
        )));
    }
    Ok(())
}

pub async fn render(
    env: &Env,
    config: &Config,
    args: OfficeRenderArgs,
    exec: Arc<ToolExecution>,
) -> Result<OfficeRenderValue, BoxError> {
    let prepared = prepare_input(env, &exec, OfficeOperationName::Render, &args.input).await?;
    let Some(attachments) = env.ctx.get::<hoh_attachment::Attachments>() else {
        return Err(box_error(ToolContextUnavailableError::new(format!(
            "cannot render \"{}\": no attachment service is mounted",
            prepared.target.display_path
        ))));
    };
    let signal = exec.signal();
    assert_render_route(
        env,
        prepared.agent.as_ref(),
        &signal,
        &prepared.target.display_path,
    )
    .await?;
    let limits = attachments.image_limits();
    let request = OfficeImagesRequest {
        extension: image_extension(&args.input),
        source: prepared.source,
        pages: args.pages.map(|pages| {
            pages
                .into_iter()
                .map(|page| u32::try_from(page).expect("pages were validated"))
                .collect()
        }),
        sheet: args.sheet,
        range: args.range,
        dpi: args
            .dpi
            .map(|dpi| u32::try_from(dpi).expect("dpi was validated")),
        max_images: config.max_images.min(limits.max_images_per_message),
        max_pixels: config.max_image_pixels.min(limits.max_image_pixels),
        max_dimension: config.max_image_dimension.min(limits.max_image_dimension),
        max_bytes: config.max_image_bytes.min(limits.max_message_image_bytes),
    };
    let result = prepared
        .kit
        .render_images(request, Some(signal))
        .await
        .map_err(|error| office_failure("render", &prepared.target.display_path, error))?;
    if result
        .images
        .iter()
        .any(|image| image.png.len() as u64 > limits.max_image_bytes)
    {
        return Err(box_error(OfficeToolError::new(
            format!(
                "cannot render \"{}\": an image exceeds the {}-byte image limit; select a smaller range or a lower dpi",
                prepared.target.display_path, limits.max_image_bytes
            ),
            OfficeToolErrorCode::OutputTooLarge,
        )));
    }
    let stem = basename(&prepared.target.display_path);
    let inputs = result
        .images
        .iter()
        .enumerate()
        .map(|(index, image)| SaveImageAttachment {
            data: image.png.clone(),
            media_type: ImageMediaType::Png,
            name: Some(format!("{stem}-{}.png", index + 1)),
        })
        .collect::<Vec<_>>();
    let refs = attachments.save_images(&inputs).await.map_err(|error| {
        if attachments.is_attachment_error(error.as_ref()) {
            let harness = HarnessError::of(error.as_ref()).expect("attachment error is coded");
            let code =
                AttachmentErrorCode::from_code(harness.code()).expect("recognized attachment code");
            box_error(
                AttachmentError::new(
                    format!(
                        "cannot render \"{}\": {}",
                        prepared.target.display_path, error
                    ),
                    code,
                )
                .with_cause(error),
            )
        } else {
            error
        }
    })?;
    if refs.len() != result.images.len() {
        return Err(format!(
            "attachment service returned {} references for {} rendered images",
            refs.len(),
            result.images.len()
        )
        .into());
    }
    let path = env.fs.process_path(&prepared.target);
    let images = result
        .images
        .into_iter()
        .zip(refs)
        .map(|(image, reference)| {
            if reference.media_type == ImageMediaType::Gif {
                return Err(format!(
                    "cannot render \"{}\": the attachment service stored a rendered PNG as GIF",
                    prepared.target.display_path
                )
                .into());
            }
            Ok(OfficeRenderedImage {
                page: image.page,
                sheet: image.sheet,
                range: image.range,
                attachment_id: reference.attachment_id.to_string(),
                media_type: reference.media_type,
                bytes: reference.bytes,
                width: reference.width,
                height: reference.height,
                original_dimensions: reference.original_dimensions,
            })
        })
        .collect::<Result<Vec<_>, BoxError>>()?;
    Ok(OfficeRenderValue {
        operation: "render".into(),
        path,
        page_count: result.page_count,
        dpi: result.dpi,
        images,
        missing_fonts: result.missing_fonts,
    })
}

fn basename(path: &str) -> &str {
    #[cfg(windows)]
    let name = path.rsplit(['/', '\\']).next();
    #[cfg(not(windows))]
    let name = path.rsplit('/').next();
    name.unwrap_or(path)
}

fn output_stem(path: &str) -> &str {
    let name = basename(path);
    name.rsplit_once('.')
        .filter(|(stem, _)| !stem.is_empty())
        .map_or(name, |(stem, _)| stem)
}

fn output_owner(env: &Env, agent: &dyn LiveAgent) -> Result<String, BoxError> {
    let own = agent.session().id().to_string();
    let Some(subagents) = env.ctx.get::<Subagents>() else {
        return Ok(own);
    };
    match subagents.root_of(agent.session()) {
        Ok(root) => Ok(root.to_string()),
        Err(error)
            if HarnessError::of(error.as_ref())
                .is_some_and(|error| error.code() == "UNAUTHORIZED") =>
        {
            Ok(own)
        }
        Err(error) => Err(error),
    }
}

async fn convert_result(
    env: &Env,
    prepared: Prepared,
    operation: &'static str,
    format: crate::content::OfficeFileFormat,
    result: Result<OfficeFileResult, SharedError>,
) -> Result<OfficeFileValue, BoxError> {
    let result =
        result.map_err(|error| office_failure(operation, &prepared.target.display_path, error))?;
    let file = env
        .fs
        .process_path_from_host_path(result.path.to_string_lossy().as_ref())
        .ok_or_else(|| {
            box_error(OfficeToolError::new(
                format!(
                    "cannot {operation} \"{}\": converted files are not available when the workspace is on a remote host",
                    prepared.target.display_path
                ),
                OfficeToolErrorCode::UnsupportedRemoteWorkspace,
            ))
        })?;
    Ok(OfficeFileValue {
        operation: operation.into(),
        path: env.fs.process_path(&prepared.target),
        file,
        format,
        bytes: result.bytes,
        page_count: result.page_count,
        missing_fonts: result.missing_fonts,
    })
}

pub async fn convert(
    env: &Env,
    args: OfficeConvertArgs,
    exec: Arc<ToolExecution>,
) -> Result<OfficeFileValue, BoxError> {
    let prepared = prepare_input(env, &exec, OfficeOperationName::Convert, &args.input).await?;
    if env
        .fs
        .process_path_from_host_path(prepared.kit.output_directory().to_string_lossy().as_ref())
        .is_none()
    {
        return Err(box_error(OfficeToolError::new(
            format!(
                "cannot convert \"{}\": converted files are not available when the workspace is on a remote host",
                prepared.target.display_path
            ),
            OfficeToolErrorCode::UnsupportedRemoteWorkspace,
        )));
    }
    let owner = output_owner(env, prepared.agent.as_ref())?;
    let name = format!(
        "{}.{}",
        output_stem(&prepared.target.display_path),
        args.format
    );
    let request = OfficeConversionRequest {
        extension: document_extension(&args.input),
        format: args.format,
        sheet: args.sheet,
        source: prepared.source.clone(),
        output: OfficeOutputTarget { owner, name },
    };
    let result = prepared.kit.convert_to(request, Some(exec.signal())).await;
    convert_result(env, prepared, "convert", args.format.into(), result).await
}

pub async fn recalculate(
    env: &Env,
    args: OfficeRecalculateArgs,
    exec: Arc<ToolExecution>,
) -> Result<OfficeFileValue, BoxError> {
    let prepared = prepare_input(env, &exec, OfficeOperationName::Recalculate, &args.input).await?;
    if env
        .fs
        .process_path_from_host_path(prepared.kit.output_directory().to_string_lossy().as_ref())
        .is_none()
    {
        return Err(box_error(OfficeToolError::new(
            format!(
                "cannot recalculate \"{}\": converted files are not available when the workspace is on a remote host",
                prepared.target.display_path
            ),
            OfficeToolErrorCode::UnsupportedRemoteWorkspace,
        )));
    }
    let owner = output_owner(env, prepared.agent.as_ref())?;
    let name = format!(
        "{}.{}",
        output_stem(&prepared.target.display_path),
        args.format
    );
    let request = OfficeRecalculationRequest {
        extension: workbook_extension(&args.input),
        format: args.format,
        source: prepared.source.clone(),
        output: OfficeOutputTarget { owner, name },
    };
    let result = prepared.kit.recalculate(request, Some(exec.signal())).await;
    convert_result(env, prepared, "recalculate", args.format.into(), result).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kit_failure(verb: &str, code: OfficeKitErrorCode) -> BoxError {
        office_failure(
            verb,
            "/w/input.docx",
            SharedError::new(box_error(OfficeKitError::new(code, "kit detail"))),
        )
    }

    #[test]
    fn classified_office_failures_keep_codes_causes_and_safe_hints() {
        for (kit_code, tool_code) in [
            (OfficeKitErrorCode::Unavailable, "OFFICE_UNAVAILABLE"),
            (OfficeKitErrorCode::Busy, "OFFICE_BUSY"),
            (OfficeKitErrorCode::InputTooLarge, "OFFICE_INPUT_TOO_LARGE"),
            (
                OfficeKitErrorCode::OutputTooLarge,
                "OFFICE_OUTPUT_TOO_LARGE",
            ),
            (
                OfficeKitErrorCode::InvalidDocument,
                "OFFICE_INVALID_DOCUMENT",
            ),
            (
                OfficeKitErrorCode::UnsupportedFormat,
                "OFFICE_UNSUPPORTED_FORMAT",
            ),
            (OfficeKitErrorCode::InvalidRequest, "OFFICE_INVALID_REQUEST"),
            (OfficeKitErrorCode::InvalidOutput, "OFFICE_INVALID_OUTPUT"),
            (OfficeKitErrorCode::Timeout, "OFFICE_TIMEOUT"),
            (OfficeKitErrorCode::SourceChanged, "OFFICE_SOURCE_CHANGED"),
            (OfficeKitErrorCode::Failed, "OFFICE_CONVERSION_FAILED"),
        ] {
            let error = kit_failure("convert", kit_code);
            let error = HarnessError::of(error.as_ref()).unwrap();
            assert_eq!(error.code(), tool_code);
            assert!(
                error.cause().is_some(),
                "classified failure keeps Kit cause"
            );
        }

        let unavailable = kit_failure("convert", OfficeKitErrorCode::Unavailable);
        let unavailable = HarnessError::of(unavailable.as_ref()).unwrap();
        assert_eq!(unavailable.name(), "OfficeToolError");
        assert_eq!(unavailable.code(), "OFFICE_UNAVAILABLE");
        assert!(
            unavailable
                .to_string()
                .contains("do not substitute system soffice")
        );
        assert!(unavailable.cause().is_some());

        let busy = kit_failure("render", OfficeKitErrorCode::Busy);
        let busy = HarnessError::of(busy.as_ref()).unwrap();
        assert_eq!(busy.code(), "OFFICE_BUSY");
        assert!(
            busy.to_string()
                .contains("Retry after other document operations finish.")
        );

        let large = kit_failure("render", OfficeKitErrorCode::OutputTooLarge);
        let large = HarnessError::of(large.as_ref()).unwrap();
        assert!(large.to_string().contains("Select fewer pages"));
    }

    #[test]
    fn conversion_output_too_large_does_not_receive_a_render_hint() {
        let error = kit_failure("convert", OfficeKitErrorCode::OutputTooLarge);
        let error = HarnessError::of(error.as_ref()).unwrap();
        assert_eq!(error.code(), "OFFICE_OUTPUT_TOO_LARGE");
        assert!(!error.to_string().contains("Select fewer pages"));
    }

    #[test]
    fn output_names_strip_only_the_admitted_final_extension() {
        #[cfg(windows)]
        assert_eq!(basename(r"C:\reports\q1.final.docx"), "q1.final.docx");
        #[cfg(not(windows))]
        assert_eq!(
            basename(r"C:\reports\q1.final.docx"),
            r"C:\reports\q1.final.docx"
        );
        #[cfg(not(windows))]
        assert_eq!(basename(r"/w/a\b.docx"), r"a\b.docx");
        assert_eq!(output_stem("/reports/q1.final.docx"), "q1.final");
        assert_eq!(output_stem("/reports/report"), "report");
    }
}
