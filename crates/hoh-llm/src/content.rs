use crate::types::*;
use hoh_attachment::{AttachmentStore, FileAttachmentRef, ImageAttachmentRef, ImageMediaType};
use indexmap::{IndexMap, IndexSet};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::borrow::Cow;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageAttachmentAccess {
    pub readonly_path: String,
}
pub type ImageAttachmentAccessResolver<'a> =
    dyn Fn(&ImageAttachmentRef) -> Option<ImageAttachmentAccess> + Send + Sync + 'a;
pub fn resolve_image_attachment_access(
    attachments: &dyn AttachmentStore,
    map_host_path: impl FnOnce(&str) -> Option<String>,
    reference: &ImageAttachmentRef,
) -> Result<Option<ImageAttachmentAccess>, hoh_error::HarnessError> {
    Ok(attachments
        .image_host_path(reference)?
        .and_then(|p| map_host_path(&p))
        .map(|readonly_path| ImageAttachmentAccess { readonly_path }))
}
fn quoted(value: &str) -> String {
    hoh_util_values::js_json_stringify(&Value::String(value.into()))
}
fn digest(id: &str) -> String {
    id.chars().skip(7).take(8).collect()
}
fn image_identity(reference: &ImageAttachmentRef) -> String {
    match &reference.name {
        None => reference.attachment_id.to_string(),
        Some(name) => format!("{} ({})", quoted(name), reference.attachment_id),
    }
}
fn normalized_access_text(
    reference: &ImageAttachmentRef,
    access: &ImageAttachmentAccess,
) -> String {
    let extension = match reference.media_type {
        ImageMediaType::Png => ".png",
        ImageMediaType::Jpeg => ".jpg",
        ImageMediaType::Webp => ".webp",
        ImageMediaType::Gif => ".gif",
    };
    format!(
        " Normalized copy (read-only; may be resized or re-encoded): {} ({}x{}px, {}). Source dimensions, format, and byte size may differ. Copy to a writable path ending in {extension} before editing.",
        quoted(&access.readonly_path),
        reference.width,
        reference.height,
        reference.media_type
    )
}
pub fn text_only_image_text(reference: &ImageAttachmentRef) -> String {
    format!(
        "[image omitted because this model accepts text only; attachment sha256:{}]",
        digest(reference.attachment_id.as_str())
    )
}
pub fn request_image_handle_text(
    reference: &ImageAttachmentRef,
    width: u64,
    height: u64,
    access: Option<&ImageAttachmentAccess>,
) -> String {
    let preview = format!(
        "Image {}; request preview {width}x{height}px.",
        image_identity(reference)
    );
    match access {
        None => {
            preview
                + " It may be resized or re-encoded; source dimensions, format, and byte size may differ."
        }
        Some(access) => preview + &normalized_access_text(reference, access),
    }
}
pub fn offloaded_image_text(
    reference: &ImageAttachmentRef,
    access: Option<&ImageAttachmentAccess>,
) -> String {
    let identity = format!(
        "image omitted to fit request image limits; {}.",
        image_identity(reference)
    );
    match access {
        None => format!(
            "[{identity} No local normalized image path is available; ask the user to attach it again if needed.]"
        ),
        Some(access) => format!("[{identity}{}]", normalized_access_text(reference, access)),
    }
}
pub fn content_has_image(content: &[ContentBlock]) -> bool {
    content.iter().any(|b| b.r#type == "image")
}
pub fn content_has_file(content: &[ContentBlock]) -> bool {
    content.iter().any(|b| b.r#type == "file")
}
pub fn file_handle_text(reference: &FileAttachmentRef, readonly_path: Option<&str>) -> String {
    file_handle_parts(
        &reference.name,
        &reference.bytes.to_string(),
        reference.attachment_id.as_str(),
        readonly_path,
    )
}
fn file_handle_parts(name: &str, bytes: &str, id: &str, readonly_path: Option<&str>) -> String {
    let identity = format!(
        "File {} ({} bytes, sha256:{})",
        quoted(name),
        bytes,
        digest(id)
    );
    match readonly_path {
        None => format!(
            "[{identity} was uploaded, but the current execution environment cannot access a readable path. Report that limitation if its contents are needed; do not claim to have read it.]"
        ),
        Some(path) => format!(
            "[{identity}: verbatim read-only copy saved at {}. Read that path with your file tools when its contents are needed; copy it to a writable location before modifying it. When delegating file work, include this saved path in the delegation prompt; only subagents sharing this execution environment can read it.]",
            quoted(path)
        ),
    }
}
fn project<'a>(
    messages: &'a [RequestMessage],
    mut replace: impl FnMut(&ContentBlock) -> Option<ContentBlock>,
) -> Cow<'a, [RequestMessage]> {
    let mut next: Option<Vec<RequestMessage>> = None;
    for (i, message) in messages.iter().enumerate() {
        let mut content: Option<Vec<ContentBlock>> = None;
        for (j, block) in message.content().iter().enumerate() {
            if let Some(block) = replace(block) {
                content
                    .get_or_insert_with(|| message.content()[..j].to_vec())
                    .push(block);
            } else if let Some(content) = &mut content {
                content.push(block.clone());
            }
        }
        if let Some(content) = content {
            next.get_or_insert_with(|| messages[..i].to_vec())
                .push(message.with_content(content));
        } else if let Some(next) = &mut next {
            next.push(message.clone());
        }
    }
    next.map_or(Cow::Borrowed(messages), Cow::Owned)
}
pub fn project_files_to_text(
    messages: &[RequestMessage],
    resolve_path: impl Fn(&FileAttachmentRef) -> Option<String>,
) -> Cow<'_, [RequestMessage]> {
    project(messages, |b| {
        if b.r#type != "file" {
            return None;
        }
        let attachment = &b.data["attachment"];
        let path = b.file().and_then(|file| resolve_path(&file.attachment));
        let name = attachment
            .get("name")
            .map_or_else(|| "undefined".into(), hoh_util_values::js_string);
        let bytes = attachment
            .get("bytes")
            .map_or_else(|| "undefined".into(), hoh_util_values::js_string);
        let id = attachment["attachmentId"].as_str().unwrap_or_default();
        Some(ContentBlock::text(file_handle_parts(
            &name,
            &bytes,
            id,
            path.as_deref(),
        )))
    })
}
pub fn project_offloaded_images(
    messages: &[RequestMessage],
    placeholder: impl Fn(&ImageAttachmentRef) -> String,
) -> Cow<'_, [RequestMessage]> {
    project(messages, |b| {
        b.image()
            .filter(|i| i.offloaded == Some(true))
            .map(|i| ContentBlock::text(placeholder(&i.attachment)))
    })
}
pub fn project_images_for_text_model(messages: &[RequestMessage]) -> Cow<'_, [RequestMessage]> {
    project(messages, |b| {
        (b.r#type == "image").then(|| {
            ContentBlock::text(format!(
                "[image omitted because this model accepts text only; attachment sha256:{}]",
                digest(
                    b.data["attachment"]["attachmentId"]
                        .as_str()
                        .unwrap_or_default()
                )
            ))
        })
    })
}
pub fn required_image_offload(
    messages: &[RequestMessage],
    budget: &LlmImageRequestBudget,
    version_bytes: impl Fn(&ImageBlock) -> u64,
) -> u64 {
    let lengths: Vec<_> = messages
        .iter()
        .flat_map(|m| m.content())
        .filter_map(ContentBlock::image)
        .filter(|b| b.offloaded != Some(true))
        .map(|b| {
            let bytes = version_bytes(&b);
            match budget.representation {
                ImageRepresentation::Raw => bytes,
                ImageRepresentation::Base64 => bytes.div_ceil(3) * 4,
            }
        })
        .collect();
    let total: u64 = lengths.iter().sum();
    let excess_count = budget
        .max_images
        .map_or(0, |max| (lengths.len() as u64).saturating_sub(max));
    let excess_bytes = budget.max_bytes.map_or(0, |max| total.saturating_sub(max));
    if excess_count == 0 && excess_bytes == 0 {
        return 0;
    }
    let count_quantum = budget.count_quantum.unwrap_or(1).max(1);
    let byte_quantum = budget.byte_quantum.unwrap_or(1).max(1);
    let remove_count = excess_count.div_ceil(count_quantum) * count_quantum;
    let remove_bytes = excess_bytes.div_ceil(byte_quantum) * byte_quantum;
    let mut count = 0;
    let mut removed_bytes = 0;
    for bytes in lengths {
        let byte_met = remove_bytes == 0
            || if byte_quantum == 1 {
                removed_bytes >= remove_bytes
            } else {
                removed_bytes > remove_bytes
            };
        if count >= remove_count && byte_met {
            break;
        }
        count += 1;
        removed_bytes += bytes;
    }
    count
}
#[derive(Debug)]
pub struct ProjectedToolUpdates<'a> {
    pub messages: Cow<'a, [RequestMessage]>,
    pub tools: Option<Cow<'a, [ToolSchema]>>,
}
fn without_developer_messages(messages: &[RequestMessage]) -> Cow<'_, [RequestMessage]> {
    if messages.iter().any(|m| m.role() == "developer") {
        Cow::Owned(
            messages
                .iter()
                .filter(|m| m.role() != "developer")
                .cloned()
                .collect(),
        )
    } else {
        Cow::Borrowed(messages)
    }
}
fn rebase_addition_only_history(
    history: &ToolHistory,
    messages: &IndexMap<&MessageId, &[ContentBlock]>,
) -> Result<ToolHistory, crate::error::ValueError> {
    let mut available: IndexMap<_, _> = history
        .tools
        .iter()
        .map(|t| (t.name.clone(), t.clone()))
        .collect();
    let mut baseline = history.tools.clone();
    let mut retained_from = 0;
    for (i, update) in history.updates.iter().enumerate() {
        let additions: IndexMap<_, _> = update
            .additions
            .iter()
            .map(|t| (t.name.as_str(), t))
            .collect();
        let mut reset = available.is_empty() && !additions.is_empty();
        // The source requires Session to resolve every referenced addition.
        for block in messages[&update.message_id] {
            match (block.r#type.as_str(), block.tool_name()) {
                ("tool-addition", Some(name)) => {
                    let tool = additions.get(name).ok_or_else(|| crate::error::ValueError::Message("Cannot destructure property 'deferLoading' of 'additions.get(...)' as it is undefined.".into()))?;
                    let mut tool = (*tool).clone();
                    tool.defer_loading = None;
                    available.insert(name.to_owned(), tool);
                }
                ("tool-removal", Some(name)) => {
                    reset = true;
                    available.shift_remove(name);
                }
                _ => {}
            }
        }
        if reset {
            baseline = available.values().cloned().collect();
            retained_from = i + 1;
        }
    }
    Ok(ToolHistory {
        tools: baseline,
        updates: history.updates[retained_from..].to_vec(),
    })
}
pub fn project_tool_updates<'a>(
    messages: &'a [RequestMessage],
    tools: Option<&'a [ToolSchema]>,
    tool_update: Option<&str>,
    history: Option<&ToolHistory>,
) -> Result<ProjectedToolUpdates<'a>, crate::error::ValueError> {
    let Some(mode) = tool_update else {
        let tools = tools.map(|tools| {
            if tools.iter().any(|t| t.defer_loading == Some(true)) {
                Cow::Owned(
                    tools
                        .iter()
                        .map(|t| ToolSchema {
                            defer_loading: None,
                            ..t.clone()
                        })
                        .collect(),
                )
            } else {
                Cow::Borrowed(tools)
            }
        });
        return Ok(ProjectedToolUpdates {
            messages: without_developer_messages(messages),
            tools,
        });
    };
    let Some(history) = history else {
        return Ok(ProjectedToolUpdates {
            messages: without_developer_messages(messages),
            tools: tools.map(Cow::Borrowed),
        });
    };
    let updates: IndexMap<_, _> = messages
        .iter()
        .filter(|m| m.role() == "developer")
        .filter_map(|m| m.id().map(|id| (id, m.content())))
        .collect();
    if history
        .updates
        .iter()
        .any(|u| !updates.contains_key(&u.message_id))
    {
        return Ok(ProjectedToolUpdates {
            messages: without_developer_messages(messages),
            tools: tools.map(Cow::Borrowed),
        });
    }
    let rebased;
    let projected = if mode == "addition-only" {
        rebased = rebase_addition_only_history(history, &updates)?;
        &rebased
    } else {
        history
    };
    let mut declarations: IndexMap<_, _> = projected
        .tools
        .iter()
        .map(|t| (t.name.clone(), t.clone()))
        .collect();
    for update in &projected.updates {
        for tool in &update.additions {
            declarations
                .entry(tool.name.clone())
                .or_insert_with(|| ToolSchema {
                    defer_loading: Some(true),
                    ..tool.clone()
                });
        }
    }
    let ids: IndexSet<_> = history.updates.iter().map(|u| &u.message_id).collect();
    let retained: IndexSet<_> = projected.updates.iter().map(|u| &u.message_id).collect();
    let mut offered: IndexSet<_> = projected
        .tools
        .iter()
        .filter(|t| t.defer_loading != Some(true))
        .map(|t| t.name.clone())
        .collect();
    let mut result = Vec::new();
    let mut unchanged = true;
    for message in messages {
        if message.role() != "developer" {
            result.push(message.clone());
            continue;
        }
        if message.id().is_none_or(|id| !ids.contains(id)) {
            unchanged = false;
            continue;
        }
        let content: Vec<_> = message
            .content()
            .iter()
            .filter(|b| match (b.r#type.as_str(), b.tool_name()) {
                ("tool-addition", Some(name)) => {
                    message.id().is_some_and(|id| retained.contains(id))
                        && declarations.contains_key(name)
                        && offered.insert(name.to_owned())
                }
                ("tool-removal", Some(name)) => mode == "in-history" && offered.shift_remove(name),
                _ => true,
            })
            .cloned()
            .collect();
        if content.is_empty() {
            unchanged = false;
            continue;
        }
        if content.len() == message.content().len() {
            result.push(message.clone());
        } else {
            unchanged = false;
            result.push(message.with_content(content));
        }
    }
    Ok(ProjectedToolUpdates {
        messages: if unchanged {
            Cow::Borrowed(messages)
        } else {
            Cow::Owned(result)
        },
        tools: Some(Cow::Owned(declarations.into_values().collect())),
    })
}
