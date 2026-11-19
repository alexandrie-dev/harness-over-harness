//! Map provider-neutral history to the Messages wire request.

use crate::file_id::DeepSeekFileId;
use crate::replay::read_replay;
use crate::types::DeepSeekConnectionOptions;
use crate::wire_types::{WireMessage, WireOutputConfig, WireRequest, WireThinking, WireTool};
use base64::Engine;
use hoh_attachment::{AttachmentId, RequestImageAttachment};
use hoh_error::HarnessError;
use hoh_llm::{
    BuiltinLlmFailureCode as Code, ContentBlock, GenerateOptions, ImageAttachmentAccessResolver,
    LlmError, Message, RequestMessage, request_image_handle_text,
};
use serde_json::{Map, Value, json};
use std::collections::{HashMap, HashSet};

fn fail(message: impl Into<String>, code: Code) -> HarnessError {
    LlmError::new(message, code).expect("serialization diagnostic is valid")
}
fn unsupported(kind: impl std::fmt::Display) -> HarnessError {
    fail(
        format!("DeepSeek Messages cannot represent {kind}"),
        Code::UnsupportedContent,
    )
}
fn tool_input(raw: &str) -> Value {
    serde_json::from_str(raw)
        .ok()
        .filter(Value::is_object)
        .unwrap_or_else(|| Value::Object(Map::new()))
}

fn assistant(
    message: &hoh_llm::AssistantMessage,
    model: &str,
    on_replay_degrade: Option<&mut dyn FnMut(&str)>,
) -> Result<Vec<Value>, HarnessError> {
    let replay = read_replay(message, model, on_replay_degrade);
    message.content.iter().enumerate().try_fold(Vec::new(), |mut out, (index, block)| {
        match block.r#type.as_str() {
            "text" => if let Some(text) = block.text_value().filter(|text| !text.is_empty()) { out.push(json!({"type":"text","text":text})); },
            "reasoning" => {
                let mut value = json!({"type":"thinking","thinking":block.text_value().unwrap_or_default()});
                if let Some(signature) = replay.as_ref().and_then(|blocks| blocks.get(index)).and_then(|block| block.signature.as_ref()) {
                    value.as_object_mut().expect("object").insert("signature".into(), Value::String(signature.clone()));
                }
                out.push(value);
            }
            "tool-call" => out.push(json!({
                "type":"tool_use",
                "id":block.data.get("id").and_then(Value::as_str).unwrap_or_default(),
                "name":block.data.get("name").and_then(Value::as_str).unwrap_or_default(),
                "input":tool_input(block.data.get("arguments").and_then(Value::as_str).unwrap_or_default()),
            })),
            other => return Err(unsupported(format_args!("assistant content {other}"))),
        }
        Ok(out)
    })
}

fn input(
    blocks: &[ContentBlock],
    images: &HashMap<AttachmentId, RequestImageAttachment>,
    access: &ImageAttachmentAccessResolver<'_>,
    file_ids: Option<&HashMap<AttachmentId, DeepSeekFileId>>,
) -> Result<Vec<Value>, HarnessError> {
    let mut out = Vec::new();
    for block in blocks {
        match block.r#type.as_str() {
            "text" => {
                if let Some(text) = block.text_value().filter(|text| !text.is_empty()) {
                    out.push(json!({"type":"text","text":text}));
                }
            }
            "reasoning" | "tool-call" => {}
            "image" => {
                let image = block.image().ok_or_else(|| {
                    fail(
                        "DeepSeek Messages request image is missing",
                        Code::InvalidRequest,
                    )
                })?;
                let version = images.get(&image.attachment.attachment_id).ok_or_else(|| {
                    fail(
                        "DeepSeek Messages request image is missing",
                        Code::InvalidRequest,
                    )
                })?;
                let file_id = file_ids.and_then(|ids| ids.get(&image.attachment.attachment_id));
                if file_ids.is_some() && file_id.is_none() {
                    return Err(fail(
                        "DeepSeek Messages request file id is missing",
                        Code::InvalidRequest,
                    ));
                }
                out.push(json!({"type":"text","text":request_image_handle_text(&image.attachment, version.width, version.height, access(&image.attachment).as_ref())}));
                out.push(match file_id {
                    Some(id) => json!({"type":"image","source":{"type":"file","file_id":id.as_str()}}),
                    None => json!({"type":"image","source":{"type":"base64","media_type":version.media_type.as_str(),"data":base64::engine::general_purpose::STANDARD.encode(&version.data)}}),
                });
            }
            other => {
                return Err(unsupported(format_args!(
                    "user/tool-result content {other}"
                )));
            }
        }
    }
    Ok(out)
}

pub fn serialize(
    options: &GenerateOptions,
    connection: &DeepSeekConnectionOptions,
    history: &[RequestMessage],
    images: &HashMap<AttachmentId, RequestImageAttachment>,
    access: &ImageAttachmentAccessResolver<'_>,
    mut on_replay_degrade: Option<&mut dyn FnMut(&str)>,
    file_ids: Option<&HashMap<AttachmentId, DeepSeekFileId>>,
) -> Result<WireRequest, HarnessError> {
    let catalog = connection
        .models
        .iter()
        .find(|entry| entry.id == options.model);
    let in_history =
        catalog.and_then(|model| model.system_prompt_update.as_deref()) == Some("in-history");
    let mut messages: Vec<WireMessage> = Vec::new();
    let mut history_system = None;
    let mut updates: Vec<WireMessage> = Vec::new();
    let flush = |messages: &mut Vec<WireMessage>,
                 updates: &mut Vec<WireMessage>|
     -> Result<(), HarnessError> {
        if updates.is_empty() {
            return Ok(());
        }
        if messages.last().is_none_or(|message| message.role != "user") {
            return Err(unsupported(
                "system update without a preceding user or tool-result turn",
            ));
        }
        messages.append(updates);
        Ok(())
    };
    for request in history {
        match request {
            RequestMessage::Input(input_message) => {
                let content = input(&input_message.content, images, access, file_ids)?;
                if content.is_empty() {
                    continue;
                }
                if let Some(previous) = messages.last_mut().filter(|message| message.role == "user")
                {
                    previous.content.extend(content);
                } else {
                    messages.push(WireMessage {
                        role: "user".into(),
                        content,
                    });
                }
            }
            RequestMessage::Durable(message) => match message.as_ref() {
                Message::Developer(developer) => {
                    let mut content = Vec::new();
                    for block in &developer.content {
                        match block.r#type.as_str() {
                            "text" => if let Some(text) = block.text_value().filter(|text| !text.is_empty()) { content.push(json!({"type":"text","text":text})); },
                            "tool-addition" | "tool-removal" => content.push(json!({
                                "type":if block.r#type == "tool-addition" { "tool_addition" } else { "tool_removal" },
                                "tool":{"type":"tool_reference","name":block.data.get("toolName").and_then(Value::as_str).unwrap_or_default()}
                            })),
                            other => return Err(unsupported(format_args!("developer content {other}"))),
                        }
                    }
                    if !content.is_empty() {
                        updates.push(WireMessage {
                            role: "system".into(),
                            content,
                        });
                    }
                }
                Message::System(system) => {
                    if system.content.iter().any(|block| block.r#type != "text") {
                        return Err(unsupported("non-text system message"));
                    }
                    let text = system
                        .content
                        .iter()
                        .filter_map(ContentBlock::text_value)
                        .collect::<String>();
                    if in_history && !messages.is_empty() {
                        if text.is_empty() {
                            return Err(unsupported("empty in-history system update"));
                        }
                        updates.push(WireMessage {
                            role: "system".into(),
                            content: vec![json!({"type":"text","text":text})],
                        });
                    } else {
                        history_system = Some(text);
                    }
                }
                Message::Assistant(assistant_message) => {
                    flush(&mut messages, &mut updates)?;
                    let content = if let Some(callback) = on_replay_degrade.as_mut() {
                        assistant(assistant_message, &options.model, Some(&mut **callback))?
                    } else {
                        assistant(assistant_message, &options.model, None)?
                    };
                    if content.is_empty() {
                        continue;
                    }
                    if let Some(previous) = messages
                        .last_mut()
                        .filter(|message| message.role == "assistant")
                    {
                        previous.content.extend(content);
                    } else {
                        messages.push(WireMessage {
                            role: "assistant".into(),
                            content,
                        });
                    }
                }
                Message::Tool(tool) => {
                    if tool.content.iter().any(|block| {
                        matches!(block.r#type.as_str(), "tool-addition" | "tool-removal")
                    }) {
                        return Err(unsupported("tool-change blocks outside developer messages"));
                    }
                    let mut result = json!({"type":"tool_result","tool_use_id":tool.tool_call_id.as_str(),"content":input(&tool.content, images, access, file_ids)?});
                    if let Some(is_error) = tool.is_error {
                        result
                            .as_object_mut()
                            .expect("object")
                            .insert("is_error".into(), Value::Bool(is_error));
                    }
                    if let Some(previous) =
                        messages.last_mut().filter(|message| message.role == "user")
                    {
                        previous.content.push(result);
                    } else {
                        messages.push(WireMessage {
                            role: "user".into(),
                            content: vec![result],
                        });
                    }
                }
                Message::User(user) => {
                    if user.content.iter().any(|block| {
                        matches!(block.r#type.as_str(), "tool-addition" | "tool-removal")
                    }) {
                        return Err(unsupported("tool-change blocks outside developer messages"));
                    }
                    let content = input(&user.content, images, access, file_ids)?;
                    if content.is_empty() {
                        continue;
                    }
                    if let Some(previous) =
                        messages.last_mut().filter(|message| message.role == "user")
                    {
                        previous.content.extend(content);
                    } else {
                        messages.push(WireMessage {
                            role: "user".into(),
                            content,
                        });
                    }
                }
            },
        }
    }
    flush(&mut messages, &mut updates)?;
    let mut pending = HashSet::new();
    for message in &mut messages {
        if message.role == "assistant" {
            pending.clear();
            for call in message
                .content
                .iter()
                .filter(|block| block.get("type").and_then(Value::as_str) == Some("tool_use"))
            {
                let id = call.get("id").and_then(Value::as_str).unwrap_or_default();
                if !pending.insert(id.to_owned()) {
                    return Err(fail(
                        "DeepSeek Messages duplicate tool call id",
                        Code::InvalidRequest,
                    ));
                }
            }
        } else if message.role == "user" {
            let mut results = Vec::new();
            let mut rest = Vec::new();
            for block in message.content.drain(..) {
                if block.get("type").and_then(Value::as_str) == Some("tool_result") {
                    let id = block
                        .get("tool_use_id")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    if !pending.remove(id) {
                        return Err(fail(
                            "DeepSeek Messages tool result has no matching call",
                            Code::InvalidRequest,
                        ));
                    }
                    results.push(block);
                } else {
                    rest.push(block);
                }
            }
            if !pending.is_empty() {
                return Err(fail(
                    "DeepSeek Messages tool calls need immediate results",
                    Code::InvalidRequest,
                ));
            }
            results.extend(rest);
            message.content = results;
        }
    }
    if !pending.is_empty() {
        return Err(fail(
            "DeepSeek Messages history ends with unresolved tools",
            Code::InvalidRequest,
        ));
    }
    let effort = options
        .reasoning_effort
        .as_ref()
        .map(ToString::to_string)
        .or_else(|| connection.defaults.reasoning_effort.clone())
        .unwrap_or_else(|| {
            if connection.defaults.thinking.as_deref() == Some("disabled") {
                "off".into()
            } else {
                "high".into()
            }
        });
    if !matches!(effort.as_str(), "off" | "low" | "high" | "max")
        || (connection.defaults.thinking.as_deref() == Some("disabled") && effort != "off")
    {
        return Err(fail(
            format!("DeepSeek Messages does not support reasoning effort {effort}"),
            Code::UnsupportedReasoningEffort,
        ));
    }
    let system = [options.system.as_deref(), history_system.as_deref()]
        .into_iter()
        .flatten()
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    Ok(WireRequest {
        model: options.model.clone(),
        stream: true,
        max_tokens: options
            .max_tokens
            .or_else(|| catalog.and_then(|model| model.max_tokens))
            .unwrap_or(connection.max_tokens),
        messages,
        system: (!system.is_empty()).then_some(system),
        thinking: WireThinking {
            r#type: if effort == "off" {
                "disabled"
            } else {
                "enabled"
            }
            .into(),
        },
        output_config: (effort != "off").then_some(WireOutputConfig { effort }),
        temperature: options.temperature,
        stop_sequences: options.stop.clone(),
        tools: options.tools.as_ref().map(|tools| {
            tools
                .iter()
                .map(|tool| WireTool {
                    name: tool.name.clone(),
                    description: tool.description.clone(),
                    input_schema: tool.parameters.clone(),
                    defer_loading: (tool.defer_loading == Some(true)).then_some(true),
                })
                .collect()
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Options, resolve_adapter_options};
    use hoh_llm::{
        AssistantProviderMetadata, MessageSource, NewAssistantMessage, NewUserMessage,
        ToolCallBlock, ToolResultMessageInput, create_assistant_message, create_system_message,
        create_tool_result_message, create_user_message,
    };

    fn connection() -> DeepSeekConnectionOptions {
        resolve_adapter_options(&Options::default()).unwrap()
    }

    fn options(model: &str, messages: Vec<RequestMessage>) -> GenerateOptions {
        GenerateOptions {
            provider: "deepseek".into(),
            model: model.into(),
            messages,
            ..Default::default()
        }
    }

    fn user(text: &str) -> RequestMessage {
        create_user_message(NewUserMessage {
            content: vec![ContentBlock::text(text)],
            source: MessageSource::new("user"),
        })
        .into()
    }

    fn serialize_text(
        options: &GenerateOptions,
        connection: &DeepSeekConnectionOptions,
        on_replay_degrade: Option<&mut dyn FnMut(&str)>,
    ) -> Result<WireRequest, HarnessError> {
        serialize(
            options,
            connection,
            &options.messages,
            &HashMap::new(),
            &|_| None,
            on_replay_degrade,
            None,
        )
    }

    #[test]
    fn places_in_history_system_updates_after_user_turn() {
        let connection = connection();
        let assistant = create_assistant_message(NewAssistantMessage {
            content: vec![ContentBlock::text("answer")],
            source: AssistantProviderMetadata {
                provider: "deepseek".into(),
                model: "deepseek-flash".into(),
                replay_state: None,
            },
        });
        let request = options(
            "deepseek-flash",
            vec![
                create_system_message("root").into(),
                user("question"),
                create_system_message("updated").into(),
                assistant.into(),
            ],
        );
        let wire = serialize_text(&request, &connection, None).unwrap();
        assert_eq!(wire.system.as_deref(), Some("root"));
        assert_eq!(
            wire.messages
                .iter()
                .map(|message| message.role.as_str())
                .collect::<Vec<_>>(),
            ["user", "system", "assistant"]
        );
        assert_eq!(wire.messages[1].content[0]["text"], "updated");
    }

    #[test]
    fn orders_tool_results_before_other_user_content() {
        let connection = connection();
        let call = create_assistant_message(NewAssistantMessage {
            content: vec![
                ToolCallBlock {
                    id: "call-1".into(),
                    name: "lookup".into(),
                    arguments: "{\"key\":1}".into(),
                }
                .into(),
            ],
            source: AssistantProviderMetadata {
                provider: "deepseek".into(),
                model: "deepseek-flash".into(),
                replay_state: None,
            },
        });
        let result = create_tool_result_message(ToolResultMessageInput {
            call_id: "call-1".into(),
            content: vec![ContentBlock::text("value")],
            is_error: false,
        });
        let request = options(
            "deepseek-flash",
            vec![call.into(), result.into(), user("continue")],
        );
        let wire = serialize_text(&request, &connection, None).unwrap();
        assert_eq!(wire.messages[1].content[0]["type"], "tool_result");
        assert_eq!(wire.messages[1].content[1]["text"], "continue");
    }

    #[test]
    fn rejects_unresolved_and_duplicate_tool_calls() {
        for calls in [
            vec![("call-1", "one")],
            vec![("call-1", "one"), ("call-1", "two")],
        ] {
            let connection = connection();
            let assistant = create_assistant_message(NewAssistantMessage {
                content: calls
                    .into_iter()
                    .map(|(id, name)| {
                        ContentBlock::from(ToolCallBlock {
                            id: id.into(),
                            name: name.into(),
                            arguments: "{}".into(),
                        })
                    })
                    .collect(),
                source: AssistantProviderMetadata {
                    provider: "deepseek".into(),
                    model: "deepseek-flash".into(),
                    replay_state: None,
                },
            });
            let request = options("deepseek-flash", vec![assistant.into()]);
            assert_eq!(
                serialize_text(&request, &connection, None)
                    .unwrap_err()
                    .code(),
                "INVALID_REQUEST"
            );
        }
    }

    #[test]
    fn reports_invalid_replay_metadata_and_keeps_durable_content() {
        let connection = connection();
        let assistant = create_assistant_message(NewAssistantMessage {
            content: vec![ContentBlock::reasoning("thought")],
            source: AssistantProviderMetadata {
                provider: "deepseek".into(),
                model: "deepseek-flash".into(),
                replay_state: Some(json!({"response":{"kind":"wrong","version":1}})),
            },
        });
        let request = options("deepseek-flash", vec![assistant.into()]);
        let mut diagnostics = Vec::new();
        let wire = serialize_text(
            &request,
            &connection,
            Some(&mut |reason| diagnostics.push(reason.to_owned())),
        )
        .unwrap();
        assert_eq!(wire.messages[0].content[0]["thinking"], "thought");
        assert_eq!(diagnostics.len(), 1);
        assert!(diagnostics[0].contains("unsupported kind or version"));
    }
}
