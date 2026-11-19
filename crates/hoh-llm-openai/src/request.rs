//! `GenerateOptions` → Chat Completions request body.
use crate::compat::{ThinkingControl, effort_ids, other_limit_field};
use crate::config::{OpenAiConfig, ToolChoice};
use crate::error::failure;
use hoh_attachment::{AttachmentStore, ImageRequestTarget, RequestImageAttachment};
use hoh_error::HarnessError;
use hoh_llm::{
    BuiltinLlmFailureCode as Code, ContentBlock, GenerateOptions, ImageBlock, RequestMessage,
    offloaded_image_text, request_image_handle_text,
};
use serde_json::{Map, Value, json};
use std::collections::HashSet;
use std::sync::Arc;

fn unsupported(what: &str) -> HarnessError {
    failure(
        Code::UnsupportedContent,
        format!("OpenAI-compatible request cannot represent {what}"),
    )
}

fn invalid(what: &str) -> HarnessError {
    failure(
        Code::InvalidRequest,
        format!("OpenAI-compatible request: {what}"),
    )
}

/// Text of a message that may only contain text (and ignorable reasoning).
fn plain_text(blocks: &[ContentBlock], role: &str) -> Result<String, HarnessError> {
    let mut parts = Vec::new();
    for block in blocks {
        match block.r#type.as_str() {
            "text" => parts.extend(block.text_value()),
            "reasoning" => {}
            other => return Err(unsupported(&format!("{role} content {other}"))),
        }
    }
    Ok(parts.join("\n\n"))
}

type Images = Vec<(ImageBlock, Option<RequestImageAttachment>)>;

/// Read the bytes of every retained image once, before any JSON is built.
async fn read_images(
    options: &GenerateOptions,
    store: Option<&Arc<dyn AttachmentStore>>,
) -> Result<Images, HarnessError> {
    let mut images = Vec::new();
    for message in &options.messages {
        for block in message.content() {
            let Some(image) = block.image() else { continue };
            if image.offloaded == Some(true) {
                images.push((image, None));
                continue;
            }
            let store = store.ok_or_else(|| {
                unsupported("an image: no attachment store is configured for this adapter")
            })?;
            let reference = &image.attachment;
            let target = ImageRequestTarget {
                width: reference.width,
                height: reference.height,
                max_bytes: reference.bytes,
            };
            let version = store
                .read_image_request(reference, &target, options.signal.as_ref())
                .await
                .map_err(|e| {
                    failure(
                        Code::InvalidRequest,
                        format!("OpenAI-compatible request image is unavailable: {e}"),
                    )
                })?;
            images.push((image, Some(version)));
        }
    }
    Ok(images)
}

fn image_parts(image: &ImageBlock, version: Option<&RequestImageAttachment>) -> Vec<Value> {
    match version {
        None => {
            vec![json!({"type": "text", "text": offloaded_image_text(&image.attachment, None)})]
        }
        Some(version) => vec![
            json!({"type": "text", "text": request_image_handle_text(
                &image.attachment, version.width, version.height, None)}),
            json!({"type": "image_url", "image_url": {"url": format!(
                "data:{};base64,{}", version.media_type, hoh_util_crypto::bytes_to_base64(&version.data))}}),
        ],
    }
}

/// Text blocks and images of a user or tool message as content parts.
fn parts(
    blocks: &[ContentBlock],
    images: &mut std::slice::Iter<'_, (ImageBlock, Option<RequestImageAttachment>)>,
    role: &str,
) -> Result<Vec<Value>, HarnessError> {
    let mut parts = Vec::new();
    for block in blocks {
        match block.r#type.as_str() {
            "text" => {
                if let Some(text) = block.text_value().filter(|t| !t.is_empty()) {
                    parts.push(json!({"type": "text", "text": text}));
                }
            }
            "reasoning" | "tool-call" => {}
            "image" => {
                let (image, version) = images.next().ok_or_else(|| invalid("image order"))?;
                parts.extend(image_parts(image, version.as_ref()));
            }
            other => return Err(unsupported(&format!("{role} content {other}"))),
        }
    }
    Ok(parts)
}

fn user_content(parts: Vec<Value>) -> Value {
    match parts.as_slice() {
        [single] if single["type"] == "text" => single["text"].clone(),
        _ => Value::Array(parts),
    }
}

/// Arguments that are not a JSON object cannot be replayed; the durable history is untouched.
fn history_arguments(raw: &str) -> String {
    match serde_json::from_str::<Value>(raw) {
        Ok(Value::Object(_)) => raw.to_owned(),
        _ => "{}".to_owned(),
    }
}

fn thinking_fields(
    config: &OpenAiConfig,
    effort: Option<&str>,
    body: &mut Map<String, Value>,
) -> Result<(), HarnessError> {
    let Some(effort) = effort else { return Ok(()) };
    let control = config.compat().thinking_control;
    if !effort_ids(control).iter().any(|(id, _)| *id == effort) {
        return Err(failure(
            Code::UnsupportedReasoningEffort,
            format!(
                "OpenAI-compatible server {:?} does not support reasoning effort {effort}",
                config.server
            ),
        ));
    }
    match control {
        ThinkingControl::Unsupported => {}
        ThinkingControl::ChatTemplateKwargs => {
            body.insert(
                "chat_template_kwargs".into(),
                json!({"enable_thinking": effort != "off"}),
            );
        }
        ThinkingControl::ReasoningEffort => {
            let value = if effort == "off" { "none" } else { effort };
            body.insert("reasoning_effort".into(), json!(value));
        }
    }
    Ok(())
}

/// Name of the output-limit field this request carries.
pub fn limit_field(config: &OpenAiConfig, swapped: bool) -> &'static str {
    let field = config.compat().max_tokens_field;
    if swapped {
        other_limit_field(field)
    } else {
        field
    }
}

/// Move the output limit to its other spelling (after the server refused the first one).
pub fn swap_limit_field(body: &mut Value, from: &str, to: &str) {
    if let Some(object) = body.as_object_mut()
        && let Some(limit) = object.remove(from)
    {
        object.insert(to.to_owned(), limit);
    }
}

pub async fn build_body(
    config: &OpenAiConfig,
    options: &GenerateOptions,
    store: Option<&Arc<dyn AttachmentStore>>,
    swapped_limit: bool,
) -> Result<Value, HarnessError> {
    let images = read_images(options, store).await?;
    let mut image_iter = images.iter();
    let mut system: Vec<String> = options
        .system
        .iter()
        .filter(|s| !s.is_empty())
        .cloned()
        .collect();
    let mut messages: Vec<Value> = Vec::new();
    // Images of tool results ride in a user message after the run of tool messages.
    let mut tool_images: Vec<Value> = Vec::new();
    let mut pending: HashSet<String> = HashSet::new();
    for message in &options.messages {
        let role = message.role();
        if role != "tool" && !tool_images.is_empty() {
            messages.push(json!({"role": "user", "content": std::mem::take(&mut tool_images)}));
        }
        match role {
            // Qwen chat templates accept one system message, first: all system text is merged.
            "system" => {
                let text = plain_text(message.content(), "system")?;
                if !text.is_empty() {
                    system.push(text);
                }
            }
            "user" => {
                if !pending.is_empty() {
                    return Err(invalid("tool calls need immediate results"));
                }
                let content = parts(message.content(), &mut image_iter, "user")?;
                if !content.is_empty() {
                    messages.push(json!({"role": "user", "content": user_content(content)}));
                }
            }
            "assistant" => {
                if !pending.is_empty() {
                    return Err(invalid("tool calls need immediate results"));
                }
                let mut text = Vec::new();
                let mut calls = Vec::new();
                for block in message.content() {
                    match block.r#type.as_str() {
                        "text" => text.extend(block.text_value().filter(|t| !t.is_empty())),
                        "reasoning" => {}
                        "tool-call" => {
                            let call = block_tool_call(block)?;
                            if !pending.insert(call.0.clone()) {
                                return Err(invalid("duplicate tool call id"));
                            }
                            calls.push(json!({"id": call.0, "type": "function",
                                "function": {"name": call.1, "arguments": history_arguments(&call.2)}}));
                        }
                        other => return Err(unsupported(&format!("assistant content {other}"))),
                    }
                }
                if text.is_empty() && calls.is_empty() {
                    continue;
                }
                let mut wire = json!({"role": "assistant", "content": text.join("\n\n")});
                if !calls.is_empty() {
                    wire["tool_calls"] = Value::Array(calls);
                }
                messages.push(wire);
            }
            "tool" => {
                let RequestMessage::Durable(durable) = message else {
                    return Err(invalid("tool result without a durable message"));
                };
                let hoh_llm::Message::Tool(result) = durable.as_ref() else {
                    return Err(invalid("tool role without a tool result"));
                };
                let call_id = result.tool_call_id.as_str();
                if !pending.remove(call_id) {
                    return Err(invalid("tool result has no matching call"));
                }
                let mut texts = Vec::new();
                for block in &result.content {
                    match block.r#type.as_str() {
                        "text" => texts.extend(block.text_value()),
                        "reasoning" | "tool-call" => {}
                        "image" => {
                            let (image, version) =
                                image_iter.next().ok_or_else(|| invalid("image order"))?;
                            tool_images.extend(image_parts(image, version.as_ref()));
                        }
                        other => return Err(unsupported(&format!("tool result content {other}"))),
                    }
                }
                messages.push(
                    json!({"role": "tool", "tool_call_id": call_id, "content": texts.join("\n")}),
                );
            }
            "developer" => {
                return Err(unsupported(
                    "a developer message (the model declares no in-history updates)",
                ));
            }
            other => return Err(unsupported(&format!("role {other}"))),
        }
    }
    if !pending.is_empty() {
        return Err(invalid("history ends with unresolved tool calls"));
    }
    if !tool_images.is_empty() {
        messages.push(json!({"role": "user", "content": tool_images}));
    }
    if !system.is_empty() {
        messages.insert(0, json!({"role": "system", "content": system.join("\n\n")}));
    }

    let compat = config.compat();
    let mut body = Map::new();
    body.insert("model".into(), json!(options.model));
    body.insert("messages".into(), Value::Array(messages));
    body.insert("stream".into(), json!(true));
    if compat.stream_usage_option {
        body.insert("stream_options".into(), json!({"include_usage": true}));
    }
    if let Some(temperature) = options.temperature {
        body.insert("temperature".into(), json!(temperature));
    }
    if let Some(max_tokens) = options.max_tokens {
        body.insert(limit_field(config, swapped_limit).into(), json!(max_tokens));
    }
    if let Some(stop) = options.stop.as_ref().filter(|s| !s.is_empty()) {
        body.insert("stop".into(), json!(stop));
    }
    if let Some(tools) = options.tools.as_ref().filter(|t| !t.is_empty()) {
        let wire: Vec<Value> = tools
            .iter()
            .map(|tool| {
                json!({"type": "function", "function": {
                "name": tool.name, "description": tool.description, "parameters": tool.parameters}})
            })
            .collect();
        body.insert("tools".into(), Value::Array(wire));
        body.insert(
            "tool_choice".into(),
            match &config.tool_choice {
                ToolChoice::Auto => json!("auto"),
                ToolChoice::Required => json!("required"),
                ToolChoice::None => json!("none"),
                ToolChoice::Function(name) => {
                    json!({"type": "function", "function": {"name": name}})
                }
            },
        );
    }
    thinking_fields(
        config,
        options.reasoning_effort.as_ref().map(|e| e.as_str()),
        &mut body,
    )?;
    Ok(Value::Object(body))
}

fn block_tool_call(block: &ContentBlock) -> Result<(String, String, String), HarnessError> {
    let field = |key: &str| {
        block
            .data
            .get(key)
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| invalid("malformed tool-call block"))
    };
    let id = field("id")?;
    if id.is_empty() {
        return Err(invalid("empty tool call id"));
    }
    Ok((id, field("name")?, field("arguments")?))
}
