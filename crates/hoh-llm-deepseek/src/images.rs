//! Deterministic request-image preparation helpers.

use crate::request_files::{ImageWireLocation, RequestFileError, RequestFiles};
use crate::types::DeepSeekConnectionOptions;
use crate::{DeepSeekFileId, resolve_request_image_target};
use hoh_attachment::{AttachmentId, AttachmentStore, RequestImageAttachment};
use hoh_error::{HarnessError, SharedError};
use hoh_llm::{
    BuiltinLlmFailureCode as Code, ImageAttachmentAccessResolver, ImageRepresentation, LlmError,
    LlmErrorOptions, LlmImageRequestBudget, RequestMessage, offloaded_image_text,
    project_offloaded_images, required_image_offload,
};
use hoh_timeout::AbortSignal;
use std::collections::HashMap;

pub async fn prepare_images(
    history: &[RequestMessage],
    connection: &DeepSeekConnectionOptions,
    model_id: &str,
    attachments: Option<&dyn AttachmentStore>,
    access: &ImageAttachmentAccessResolver<'_>,
    signal: Option<&AbortSignal>,
) -> Result<
    (
        Vec<RequestMessage>,
        HashMap<AttachmentId, RequestImageAttachment>,
    ),
    HarnessError,
> {
    let messages = project_offloaded_images(history, |reference| {
        offloaded_image_text(reference, access(reference).as_ref())
    })
    .into_owned();
    let mut versions = HashMap::new();
    let model = connection.models.iter().find(|entry| entry.id == model_id);
    let has_images = messages
        .iter()
        .flat_map(RequestMessage::content)
        .any(|block| block.r#type == "image");
    if !has_images {
        return Ok((messages, versions));
    }
    let (Some(model), Some(attachments)) = (
        model.filter(|entry| {
            entry
                .input_modalities
                .as_ref()
                .is_some_and(|items| items.iter().any(|item| item == "image"))
        }),
        attachments,
    ) else {
        return Err(LlmError::new(
            "DeepSeek Messages image input requires a vision model and attachment service",
            Code::UnsupportedContent,
        )
        .expect("image diagnostic is valid"));
    };
    for message in &messages {
        if !matches!(message.role(), "user" | "tool")
            && message
                .content()
                .iter()
                .any(|block| block.r#type == "image")
        {
            return Err(LlmError::new(
                "DeepSeek Messages supports images only in user messages and tool results",
                Code::UnsupportedContent,
            )
            .expect("image role diagnostic is valid"));
        }
        for image in message
            .content()
            .iter()
            .filter_map(hoh_llm::ContentBlock::image)
        {
            if versions.contains_key(&image.attachment.attachment_id) {
                continue;
            }
            let target = resolve_request_image_target(model, &image.attachment);
            let version = attachments
                .read_image_request(&image.attachment, &target, signal)
                .await
                .map_err(|error| {
                    LlmError::with_options(
                        "DeepSeek Messages image preparation failed",
                        Code::UnsupportedContent,
                        LlmErrorOptions {
                            cause: Some(SharedError::new(error)),
                            ..Default::default()
                        },
                    )
                    .expect("image preparation diagnostic is valid")
                })?;
            versions.insert(image.attachment.attachment_id, version);
        }
    }
    assert_images_fit(&messages, &versions, connection, ImageRepresentation::Raw)?;
    Ok((messages, versions))
}

pub fn inline_images(
    messages: &[RequestMessage],
    versions: &HashMap<AttachmentId, RequestImageAttachment>,
    connection: &DeepSeekConnectionOptions,
) -> Result<Vec<RequestMessage>, HarnessError> {
    assert_images_fit(messages, versions, connection, ImageRepresentation::Base64)?;
    Ok(messages.to_vec())
}

fn assert_images_fit(
    messages: &[RequestMessage],
    versions: &HashMap<AttachmentId, RequestImageAttachment>,
    connection: &DeepSeekConnectionOptions,
    representation: ImageRepresentation,
) -> Result<(), HarnessError> {
    for image in messages
        .iter()
        .flat_map(RequestMessage::content)
        .filter_map(hoh_llm::ContentBlock::image)
    {
        if !versions.contains_key(&image.attachment.attachment_id) {
            return Err(LlmError::new(
                "DeepSeek Messages request image is missing",
                Code::InvalidRequest,
            )
            .expect("image diagnostic is valid"));
        }
    }
    let raw = matches!(representation, ImageRepresentation::Raw);
    let budget = LlmImageRequestBudget {
        representation,
        max_bytes: Some(if raw {
            connection.max_request_files_bytes
        } else {
            connection.max_inline_request_image_bytes
        }),
        max_images: Some(connection.max_images_per_request),
        byte_quantum: Some(if raw {
            connection.image_offload_byte_quantum
        } else {
            connection.inline_image_offload_byte_quantum
        }),
        count_quantum: Some(connection.image_offload_count_quantum),
    };
    let offload_images = required_image_offload(messages, &budget, |image| {
        versions
            .get(&image.attachment.attachment_id)
            .map_or(0, |version| version.bytes)
    });
    if offload_images == 0 {
        return Ok(());
    }
    let representation = if raw { "raw" } else { "base64" };
    Err(LlmError::with_options(
        format!(
            "DeepSeek Messages {representation} request images exceed the route budget; {offload_images} more oldest occurrence(s) must be offloaded."
        ),
        Code::ImageOffloadRequired,
        LlmErrorOptions {
            offload_images: Some(offload_images),
            ..Default::default()
        },
    )
    .expect("image offload diagnostic is valid"))
}

pub async fn prepare_file_ids(
    messages: &[RequestMessage],
    versions: &HashMap<AttachmentId, RequestImageAttachment>,
    files: &mut RequestFiles<'_>,
) -> Result<HashMap<AttachmentId, DeepSeekFileId>, RequestFileError> {
    let mut ids = HashMap::new();
    for (message_index, message) in messages.iter().enumerate() {
        for (image_index, image) in message
            .content()
            .iter()
            .filter_map(hoh_llm::ContentBlock::image)
            .enumerate()
        {
            let version = versions
                .get(&image.attachment.attachment_id)
                .ok_or_else(RequestFileError::missing_image)?;
            let id = files
                .resolve(
                    version,
                    ImageWireLocation {
                        message: message_index + 1,
                        image: image_index + 1,
                    },
                )
                .await?;
            ids.insert(image.attachment.attachment_id, id);
        }
    }
    Ok(ids)
}
pub fn file_ids_complete(
    messages: &[RequestMessage],
    ids: &HashMap<AttachmentId, DeepSeekFileId>,
) -> bool {
    messages
        .iter()
        .flat_map(RequestMessage::content)
        .filter_map(hoh_llm::ContentBlock::image)
        .all(|image| ids.contains_key(&image.attachment.attachment_id))
}
