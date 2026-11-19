use crate::{
    diagnostic::{
        McpLocalError, McpLocalErrorCode, McpToolError, McpToolErrorCode, disconnected_message,
        mcp_request_error,
    },
    generation::McpGeneration,
    output_schema::CompiledOutputSchema,
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use hoh_attachment::{Attachments, ImageMediaType, SaveImageAttachment, is_image_admission_error};
use hoh_error::BoxError;
use hoh_kernel::{Context, Disposer, Service};
use hoh_llm::{ContentBlock, ImageBlock, Llm};
use hoh_tools::{
    ToolDefinition, ToolExecution, ToolExecutionResult, ToolOutputDefinition,
    ToolRegistrationOptions, Tools,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::{future::Future, pin::Pin, sync::Arc};

pub type McpCallFuture = Pin<Box<dyn Future<Output = Result<Value, BoxError>> + Send + 'static>>;
pub type McpCall =
    Arc<dyn Fn(Map<String, Value>, Arc<ToolExecution>) -> McpCallFuture + Send + Sync>;
pub type McpRedact = Arc<dyn Fn(&str) -> String + Send + Sync>;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpResult {
    pub content: Vec<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_content: Option<Value>,
}

pub struct McpToolDefinitionOptions {
    pub name: String,
    pub raw_name: String,
    pub description: String,
    pub input_schema: Map<String, Value>,
    pub output_schema: Option<Value>,
    pub task_required: bool,
    pub redact: Option<McpRedact>,
    pub call: McpCall,
}

pub fn public_tool_name(server_name: &str, raw_name: &str) -> String {
    let joined = format!("mcp__{server_name}__{raw_name}");
    let normalized: String = joined
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-') {
                ch
            } else {
                '_'
            }
        })
        .collect();
    if normalized == joined && normalized.len() <= 64 {
        return normalized;
    }
    let hash = Sha256::digest(format!("{server_name}\0{raw_name}").as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!(
        "{}_{}",
        &normalized[..normalized.len().min(51)],
        &hash[..12]
    )
}

pub fn create_mcp_tool_definition(
    ctx: &Context,
    options: McpToolDefinitionOptions,
) -> Result<ToolDefinition, BoxError> {
    let McpToolDefinitionOptions {
        name,
        raw_name,
        description,
        input_schema,
        output_schema,
        task_required,
        redact,
        call,
    } = options;
    let output_check = output_schema.clone().map(CompiledOutputSchema::compile);
    let structured_schema =
        output_schema.filter(|schema| hoh_tools::assert_supported_json_schema(schema).is_ok());
    let required = if structured_schema.is_some() {
        vec![json!("content"), json!("structuredContent")]
    } else {
        vec![json!("content")]
    };
    let output_schema = json!({
        "type": "object",
        "properties": { "content": { "type": "array", "items": {} }, "structuredContent": structured_schema.unwrap_or_else(|| json!({})) },
        "required": required,
        "additionalProperties": false
    });
    let render_name = raw_name.clone();
    let output = ToolOutputDefinition {
        schema: output_schema,
        render: Arc::new(move |_args, value| {
            let result: McpResult = serde_json::from_value(value.clone())?;
            Ok(vec![ContentBlock::text(extract_text(
                &result.content,
                &render_name,
            ))])
        }),
        presentation_meta: None,
    };
    let execute_name = raw_name.clone();
    let projection_ctx = ctx.clone();
    let projections = Arc::new(std::sync::Mutex::new(std::collections::HashMap::<
        usize,
        PreparedProjection,
    >::new()));
    let execute_projections = projections.clone();
    let execute = Arc::new(move |args: Value, execution: Arc<ToolExecution>| {
        let call = call.clone();
        let raw_name = execute_name.clone();
        let output_check = output_check.clone();
        let redact = redact.clone();
        let ctx = projection_ctx.clone();
        let projections = execute_projections.clone();
        Box::pin(async move {
            if task_required {
                return Err(Box::new(McpToolError::new(
                    format!(
                        "Tool \"{raw_name}\" requires task-based execution, which this bridge does not support"
                    ),
                    McpToolErrorCode::UnsupportedTaskExecution,
                )) as BoxError);
            }
            if let Some(Err(error)) = output_check.as_ref() {
                let message = format!(
                    "Tool \"{raw_name}\" returned an invalid MCP result: its output schema cannot be compiled: {error}"
                );
                return Err(Box::new(McpToolError::new(
                    redact
                        .as_deref()
                        .map_or(message.clone(), |redact| redact(&message)),
                    McpToolErrorCode::InvalidResult,
                )) as BoxError);
            }
            let args = args.as_object().cloned().unwrap_or_default();
            let raw = call(args, execution.clone()).await?;
            let result: rmcp::model::CallToolResult =
                serde_json::from_value(raw).map_err(|error| {
                    let message =
                        format!("Tool \"{raw_name}\" returned an invalid MCP result: {error}");
                    Box::new(McpToolError::with_cause(
                        redact
                            .as_deref()
                            .map_or(message.clone(), |redact| redact(&message)),
                        McpToolErrorCode::InvalidResult,
                        error,
                    )) as BoxError
                })?;
            let content = result
                .content
                .into_iter()
                .map(serde_json::to_value)
                .collect::<Result<Vec<_>, _>>()?;
            if result.is_error == Some(true) {
                let message = extract_text(&content, &raw_name);
                return Err(Box::new(McpToolError::new(
                    redact
                        .as_deref()
                        .map_or(message.clone(), |redact| redact(&message)),
                    McpToolErrorCode::ToolError,
                )) as BoxError);
            }
            if let Some(check) = output_check {
                let schema = check.expect("compile failure returned before provider call");
                let structured = result.structured_content.as_ref().ok_or_else(|| {
                    let message = format!("Tool \"{raw_name}\" returned an invalid MCP result: the tool has an output schema but returned no structured content");
                    Box::new(McpToolError::new(
                        redact.as_deref().map_or(message.clone(), |redact| redact(&message)),
                        McpToolErrorCode::InvalidResult,
                    )) as BoxError
                })?;
                if let Some(failure) = schema.validate(structured) {
                    let message = format!(
                        "Tool \"{raw_name}\" returned an invalid MCP result: structured content does not match the output schema: {failure}"
                    );
                    return Err(Box::new(McpToolError::new(
                        redact
                            .as_deref()
                            .map_or(message.clone(), |redact| redact(&message)),
                        McpToolErrorCode::InvalidResult,
                    )) as BoxError);
                }
            }
            let value = serde_json::to_value(McpResult {
                content,
                structured_content: result.structured_content,
            })?;
            if value
                .get("content")
                .and_then(Value::as_array)
                .is_some_and(|content| {
                    content
                        .iter()
                        .any(|block| block.get("type") == Some(&Value::String("image".into())))
                })
            {
                let fallback = vec![ContentBlock::text(extract_text(
                    value["content"].as_array().expect("checked array"),
                    &raw_name,
                ))];
                let projected = prepare_image_projection(
                    &ctx,
                    &execution,
                    value["content"].as_array().expect("checked array"),
                    &raw_name,
                )
                .await;
                lock_map(&projections).insert(
                    Arc::as_ptr(&execution) as usize,
                    PreparedProjection {
                        value: value.clone(),
                        fallback,
                        content: projected,
                    },
                );
            }
            Ok(value)
        }) as hoh_tools::shim::BoxFuture<'static, Result<Value, BoxError>>
    });
    let mut definition = ToolDefinition::new(name, description, input_schema, output, execute);
    definition.project_content = Some(Arc::new(
        move |execution: &ToolExecution, result: &ToolExecutionResult| {
            let projection =
                lock_map(&projections).remove(&(std::ptr::from_ref(execution) as usize))?;
            if result.is_error()
                || result.value() != Some(&projection.value)
                || result.content() != projection.fallback
            {
                return None;
            }
            Some(projection.content)
        },
    ));
    Ok(definition)
}

pub async fn definitions_from_generation(
    ctx: &Context,
    generation: &McpGeneration,
    server_name: &str,
    timeout_ms: u64,
    redact: Option<McpRedact>,
) -> Result<Vec<ToolDefinition>, BoxError> {
    let listed = generation.list_all_tools().await?;
    let mut names = std::collections::HashSet::new();
    let mut definitions = Vec::with_capacity(listed.len());
    for tool in listed {
        let raw_name = tool.name.into_owned();
        let name = public_tool_name(server_name, &raw_name);
        if !names.insert(name.clone()) {
            return Err(format!("mcp-client({server_name}): server listed tool \"{raw_name}\" more than once — invalid tool list").into());
        }
        let call_generation = generation.clone();
        let call_raw_name = raw_name.clone();
        let call_redact = redact.clone();
        let disconnected_server = server_name.to_owned();
        let call: McpCall = Arc::new(move |arguments, execution| {
            let generation = call_generation.clone();
            let raw_name = call_raw_name.clone();
            let call_redact = call_redact.clone();
            let server_name = disconnected_server.clone();
            Box::pin(async move {
                let signal = execution.signal();
                signal
                    .throw_if_aborted()
                    .map_err(|error| Box::new(error) as BoxError)?;
                let closed = tokio::select! {
                    reason = signal.cancelled() => return Err(Box::new(reason) as BoxError),
                    closed = generation.is_closed() => closed,
                };
                if closed {
                    return Err(Box::new(McpToolError::new(
                        disconnected_message(&server_name),
                        McpToolErrorCode::ServerUnavailable,
                    )) as BoxError);
                }
                let request = tokio::time::timeout(
                    std::time::Duration::from_millis(timeout_ms.max(1)),
                    generation.call_tool(raw_name.clone(), arguments),
                );
                let result = tokio::select! {
                    reason = signal.cancelled() => return Err(Box::new(reason) as BoxError),
                    result = request => result,
                }
                .map_err(|_| {
                    Box::new(McpToolError::new(
                        format!(
                            "MCP tool \"{raw_name}\" failed: request timeout after {timeout_ms}ms"
                        ),
                        McpToolErrorCode::Timeout,
                    )) as BoxError
                })?
                .map_err(|error| {
                    Box::new(mcp_request_error(
                        error,
                        &format!("tool \"{raw_name}\""),
                        call_redact.as_deref(),
                    )) as BoxError
                })?;
                let mut value = serde_json::to_value(result)?;
                if value.get("isError") == Some(&Value::Bool(true))
                    && let Some(redact) = call_redact.as_deref()
                    && let Some(content) = value.get_mut("content").and_then(Value::as_array_mut)
                {
                    *content = vec![json!({
                        "type": "text",
                        "text": redact(&extract_text(content, &raw_name)),
                    })];
                }
                Ok(value)
            })
        });
        definitions.push(create_mcp_tool_definition(
            ctx,
            McpToolDefinitionOptions {
                name,
                raw_name,
                description: tool
                    .description
                    .map(|value| value.into_owned())
                    .unwrap_or_default(),
                input_schema: (*tool.input_schema).clone(),
                output_schema: tool
                    .output_schema
                    .map(|value| Value::Object((*value).clone())),
                task_required: false,
                redact: redact.clone(),
                call,
            },
        )?);
    }
    Ok(definitions)
}

pub async fn register_generation_tools(
    ctx: &Context,
    tools: &Service<Tools>,
    generation: &McpGeneration,
    server_name: &str,
    timeout_ms: u64,
    redact: Option<McpRedact>,
) -> Result<Vec<Disposer>, BoxError> {
    let definitions =
        definitions_from_generation(ctx, generation, server_name, timeout_ms, redact).await?;
    register_definitions(ctx, tools, definitions).await
}

pub async fn register_definitions(
    ctx: &Context,
    tools: &Service<Tools>,
    definitions: Vec<ToolDefinition>,
) -> Result<Vec<Disposer>, BoxError> {
    let mut disposers = Vec::with_capacity(definitions.len());
    for definition in definitions {
        match tools.register(ctx, definition, ToolRegistrationOptions::default()) {
            Ok(disposer) => disposers.push(disposer),
            Err(error) => {
                for disposer in disposers {
                    disposer.dispose().await;
                }
                return Err(Box::new(McpLocalError::with_cause(
                    "MCP tool registration failed",
                    McpLocalErrorCode::ToolRegistrationFailed,
                    error,
                )));
            }
        }
    }
    Ok(disposers)
}

struct PreparedProjection {
    value: Value,
    fallback: Vec<ContentBlock>,
    content: Vec<ContentBlock>,
}
fn lock_map<T>(value: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    value
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

async fn prepare_image_projection(
    ctx: &Context,
    execution: &ToolExecution,
    content: &[Value],
    tool_name: &str,
) -> Vec<ContentBlock> {
    let mut decoded = Vec::new();
    let mut image_indexes = Vec::new();
    let mut errors = std::collections::HashMap::new();
    for (index, value) in content.iter().enumerate() {
        if value.get("type").and_then(Value::as_str) != Some("image") {
            continue;
        }
        image_indexes.push(index);
        match decode_image(value) {
            Ok(image) => decoded.push(image),
            Err(error) => {
                errors.insert(index, error);
            }
        }
    }
    if !errors.is_empty() {
        return project_content(content, tool_name, |block, index| {
            ContentBlock::text(image_diagnostic(
                block,
                errors
                    .get(&index)
                    .map(String::as_str)
                    .unwrap_or("another image in the same result was invalid"),
            ))
        });
    }
    let attachments = match resolve_image_admission(ctx, execution).await {
        Ok(store) => store,
        Err(reason) => {
            return project_content(content, tool_name, |block, _| {
                ContentBlock::text(image_diagnostic(block, &reason))
            });
        }
    };
    match attachments.save_images(&decoded).await {
        Ok(references) => {
            let by_index = image_indexes
                .into_iter()
                .zip(references)
                .collect::<std::collections::HashMap<_, _>>();
            project_content(content, tool_name, |_block, index| {
                ImageBlock {
                    attachment: by_index[&index].clone(),
                    offloaded: None,
                }
                .into()
            })
        }
        Err(error) => {
            let reason = if is_image_admission_error(error.as_ref()) {
                format!("image admission rejected the result: {error}")
            } else {
                "durable image storage rejected the result".to_owned()
            };
            project_content(content, tool_name, |block, _| {
                ContentBlock::text(image_diagnostic(block, &reason))
            })
        }
    }
}

fn decode_image(value: &Value) -> Result<SaveImageAttachment, String> {
    let media = value
        .get("mimeType")
        .and_then(Value::as_str)
        .ok_or_else(|| "the declared media type is not PNG, JPEG, WebP, or GIF".to_owned())?;
    let media_type = match media {
        "image/png" => ImageMediaType::Png,
        "image/jpeg" => ImageMediaType::Jpeg,
        "image/webp" => ImageMediaType::Webp,
        "image/gif" => ImageMediaType::Gif,
        _ => return Err("the declared media type is not PNG, JPEG, WebP, or GIF".to_owned()),
    };
    let encoded = value
        .get("data")
        .and_then(Value::as_str)
        .ok_or_else(|| "the image data is not canonical base64".to_owned())?;
    if encoded
        .bytes()
        .any(|byte| byte.is_ascii_whitespace() || matches!(byte, b'-' | b'_'))
    {
        return Err("the image data is not canonical base64".to_owned());
    }
    let data = STANDARD
        .decode(encoded)
        .map_err(|_| "the image data is not canonical base64".to_owned())?;
    if STANDARD.encode(&data) != encoded {
        return Err("the image data is not canonical base64".to_owned());
    }
    Ok(SaveImageAttachment {
        data,
        media_type,
        name: None,
    })
}

async fn resolve_image_admission(
    ctx: &Context,
    execution: &ToolExecution,
) -> Result<Service<Attachments>, String> {
    let attachments = ctx
        .get::<Attachments>()
        .ok_or_else(|| "no attachment store is mounted".to_owned())?;
    let agent = execution
        .agent
        .as_ref()
        .ok_or_else(|| "the current model route could not be resolved".to_owned())?;
    let header = agent.session().request_header();
    let provider = header
        .as_ref()
        .and_then(|value| value.get("config"))
        .and_then(|value| value.get("provider"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| agent.options().provider());
    let model = header
        .as_ref()
        .and_then(|value| value.get("config"))
        .and_then(|value| value.get("model"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| agent.options().model());
    let (provider, model) = provider
        .zip(model)
        .ok_or_else(|| "the current model route could not be resolved".to_owned())?;
    let llm = ctx
        .get::<Llm>()
        .ok_or_else(|| "the current model route could not be resolved".to_owned())?;
    let signal = execution.signal();
    let info = llm
        .resolve_model_info(&provider, &model, Some(&signal))
        .await
        .map_err(|_| "the current model route could not be verified".to_owned())?;
    if !info
        .model
        .input_modalities
        .as_ref()
        .is_some_and(|values| values.iter().any(|value| value == "image"))
    {
        return Err(format!("model \"{model}\" does not declare image input"));
    }
    if signal.aborted() {
        return Err("the tool call was canceled before image storage".to_owned());
    }
    Ok(attachments)
}

fn image_diagnostic(block: &Value, reason: &str) -> String {
    format!(
        "[image unavailable: {}; {reason}; raw image data remains available to programmatic callers]",
        block
            .get("mimeType")
            .and_then(Value::as_str)
            .unwrap_or("unknown media type")
    )
}

fn project_content(
    content: &[Value],
    tool_name: &str,
    mut image: impl FnMut(&Value, usize) -> ContentBlock,
) -> Vec<ContentBlock> {
    let mut projected = Vec::new();
    let mut text = Vec::new();
    let flush = |projected: &mut Vec<ContentBlock>, text: &mut Vec<String>| {
        if !text.is_empty() {
            projected.push(ContentBlock::text(std::mem::take(text).join("\n")));
        }
    };
    for (index, value) in content.iter().enumerate() {
        let Some(block) = value.as_object() else {
            text.push("[unsupported MCP content block: expected an object]".to_owned());
            continue;
        };
        match block.get("type").and_then(Value::as_str).unwrap_or_default() {
            "text" => if let Some(value) = block.get("text").and_then(Value::as_str) { text.push(value.to_owned()); },
            "image" => { flush(&mut projected, &mut text); projected.push(image(value, index)); },
            "resource_link" => match (block.get("name").and_then(Value::as_str), block.get("uri").and_then(Value::as_str)) { (Some(name), Some(uri)) => text.push(format!("Resource link: {name} ({uri})")), _ => text.push("[resource link unavailable: the MCP block is missing its name or URI]".to_owned()) },
            "audio" => text.push(format!("[audio result unsupported: {}; raw audio data remains available to programmatic callers]", block.get("mimeType").and_then(Value::as_str).unwrap_or("unknown media type"))),
            "resource" => text.push("[embedded resource unsupported; raw resource data remains available to programmatic callers]".to_owned()),
            kind => text.push(format!("[unsupported MCP content type: {kind}]")),
        }
    }
    flush(&mut projected, &mut text);
    if projected.is_empty() {
        vec![ContentBlock::text(format!(
            "({tool_name} returned no model-visible content)"
        ))]
    } else {
        projected
    }
}

pub fn extract_text(content: &[Value], tool_name: &str) -> String {
    let mut parts = Vec::new();
    for value in content {
        let Some(block) = value.as_object() else {
            parts.push("[unsupported MCP content block: expected an object]".to_owned());
            continue;
        };
        match block.get("type").and_then(Value::as_str).unwrap_or_default() {
            "text" => if let Some(text) = block.get("text").and_then(Value::as_str) { parts.push(text.to_owned()); },
            "image" => parts.push(format!("[image unavailable: {}; this result was not admitted to durable model context; raw image data remains available to programmatic callers]", block.get("mimeType").and_then(Value::as_str).unwrap_or("unknown media type"))),
            "audio" => parts.push(format!("[audio result unsupported: {}; raw audio data remains available to programmatic callers]", block.get("mimeType").and_then(Value::as_str).unwrap_or("unknown media type"))),
            "resource" => parts.push("[embedded resource unsupported; raw resource data remains available to programmatic callers]".to_owned()),
            "resource_link" => match (block.get("name").and_then(Value::as_str), block.get("uri").and_then(Value::as_str)) { (Some(name), Some(uri)) => parts.push(format!("Resource link: {name} ({uri})")), _ => parts.push("[resource link unavailable: the MCP block is missing its name or URI]".to_owned()) },
            kind => parts.push(format!("[unsupported MCP content type: {kind}]")),
        }
    }
    if parts.is_empty() {
        format!("({tool_name} returned no model-visible content)")
    } else {
        parts.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hoh_kernel::Kernel;
    use hoh_timeout::AbortController;
    use hoh_tools::{
        ScheduledToolPreparation, ToolExecutionInput, ToolExecutionPipeline, ToolExecutionSource,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};

    async fn execute_definition(definition: ToolDefinition) -> ToolExecutionResult {
        let kernel = Kernel::start();
        let ctx = kernel.root();
        ctx.plugin::<hoh_system_prompt::SystemPrompt>(json!({}))
            .unwrap()
            .settled()
            .await
            .unwrap();
        ctx.plugin::<hoh_tools::ToolRuntime>(json!({}))
            .unwrap()
            .settled()
            .await
            .unwrap();
        let definition = Arc::new(definition);
        let source = ToolExecutionSource {
            get: Some(Arc::new(move |_, _, _| Some(definition.clone()))),
            reject: None,
        };
        let pipeline = ToolExecutionPipeline::with_source(ctx, source).unwrap();
        let prepared = pipeline
            .prepare(ToolExecutionInput::new(
                "call".into(),
                "mcp__test__tool",
                json!({}),
                AbortController::new().signal(),
            ))
            .await
            .unwrap();
        let exec = prepared.exec().clone();
        let ScheduledToolPreparation::Dispatch(dispatch) = prepared else {
            panic!("definition must dispatch")
        };
        let outcome = pipeline.dispatch(dispatch).await;
        let result = pipeline.complete(&exec, outcome).await;
        kernel.shutdown().await;
        result
    }

    fn definition_with(
        ctx: &Context,
        raw_name: &str,
        schema: Value,
        calls: Arc<AtomicUsize>,
        result: Value,
        redact: Option<McpRedact>,
    ) -> ToolDefinition {
        let call: McpCall = Arc::new(move |_, _| {
            calls.fetch_add(1, Ordering::SeqCst);
            let result = result.clone();
            Box::pin(async move { Ok(result) })
        });
        create_mcp_tool_definition(
            ctx,
            McpToolDefinitionOptions {
                name: "mcp__test__tool".into(),
                raw_name: raw_name.into(),
                description: String::new(),
                input_schema: Map::new(),
                output_schema: Some(schema),
                task_required: false,
                redact,
                call,
            },
        )
        .unwrap()
    }

    #[test]
    fn canonical_image_data_decodes() {
        let image = decode_image(&json!({"type":"image","mimeType":"image/png","data":"aGVsbG8="}))
            .unwrap();
        assert_eq!(image.data, b"hello");
        assert_eq!(image.media_type, ImageMediaType::Png);
    }

    #[test]
    fn base64_aliases_and_whitespace_are_rejected() {
        for data in ["aGVs bG8=", "aGVsbG8", "____"] {
            assert_eq!(
                decode_image(&json!({"type":"image","mimeType":"image/png","data":data}))
                    .unwrap_err(),
                "the image data is not canonical base64"
            );
        }
    }

    #[test]
    fn image_batch_projection_keeps_original_order() {
        let blocks = vec![
            json!({"type":"text","text":"before"}),
            json!({"type":"image","mimeType":"image/png"}),
            json!({"type":"text","text":"after"}),
        ];
        let projected = project_content(&blocks, "mixed", |_block, _| {
            ContentBlock::new("image", Map::new())
        });
        assert_eq!(
            projected
                .iter()
                .map(|block| block.r#type.as_str())
                .collect::<Vec<_>>(),
            ["text", "image", "text"]
        );
        assert_eq!(projected[0].text_value(), Some("before"));
        assert_eq!(projected[2].text_value(), Some("after"));
    }

    #[test]
    fn unsupported_image_media_is_explicit() {
        assert_eq!(
            decode_image(&json!({"type":"image","mimeType":"image/svg+xml","data":""}))
                .unwrap_err(),
            "the declared media type is not PNG, JPEG, WebP, or GIF"
        );
    }

    #[tokio::test]
    async fn pattern_properties_output_schema_accepts_matching_content() {
        let kernel = Kernel::start();
        let calls = Arc::new(AtomicUsize::new(0));
        let definition = definition_with(
            &kernel.root(),
            "tool",
            json!({"type":"object","patternProperties":{"^x-":{"type":"string"}}}),
            calls.clone(),
            serde_json::to_value(rmcp::model::CallToolResult::structured(
                json!({"x-kept":"k"}),
            ))
            .unwrap(),
            None,
        );
        let result = execute_definition(definition).await;
        kernel.shutdown().await;
        assert!(!result.is_error(), "{:?}", result.error());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn invalid_output_pattern_blocks_provider_call_and_redacts_tool_name() {
        let kernel = Kernel::start();
        let calls = Arc::new(AtomicUsize::new(0));
        let definition = definition_with(
            &kernel.root(),
            "SECRET",
            json!({"type":"string","pattern":"("}),
            calls.clone(),
            serde_json::to_value(rmcp::model::CallToolResult::structured(json!("unused"))).unwrap(),
            Some(Arc::new(|text| text.replace("SECRET", "[redacted]"))),
        );
        let result = execute_definition(definition).await;
        kernel.shutdown().await;
        let message = &result.error().expect("compile error").message;
        assert!(message.contains("cannot be compiled"), "{message}");
        assert!(!message.contains("SECRET"), "{message}");
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn structured_validation_uses_ajv_style_path() {
        let kernel = Kernel::start();
        let calls = Arc::new(AtomicUsize::new(0));
        let definition = definition_with(
            &kernel.root(),
            "tool",
            json!({"type":"object","properties":{"answer":{"type":"integer"}}}),
            calls,
            serde_json::to_value(rmcp::model::CallToolResult::structured(
                json!({"answer":"no"}),
            ))
            .unwrap(),
            None,
        );
        let result = execute_definition(definition).await;
        kernel.shutdown().await;
        assert!(
            result
                .error()
                .expect("validation error")
                .message
                .contains("data/answer must be integer")
        );
    }

    #[tokio::test]
    async fn is_error_resource_link_is_redacted_as_one_text_result() {
        let kernel = Kernel::start();
        let calls = Arc::new(AtomicUsize::new(0));
        let definition = definition_with(
            &kernel.root(),
            "tool",
            json!({}),
            calls,
            json!({
                "content":[{"type":"resource_link","name":"secret","uri":"https://h/?t=SECRET"}],
                "isError":true
            }),
            Some(Arc::new(|text| text.replace("SECRET", "[redacted]"))),
        );
        let result = execute_definition(definition).await;
        kernel.shutdown().await;
        let message = &result.error().expect("tool error").message;
        assert!(message.contains("[redacted]"), "{message}");
        assert!(!message.contains("SECRET"), "{message}");
    }
}
