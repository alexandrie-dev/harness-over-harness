//! Canonical base64 admission, followed by delegation to the deployment store.

use crate::{
    AttachmentError, AttachmentErrorCode, AttachmentStore, EncodedFileAttachment,
    EncodedImageAttachment, FileAttachmentRef, ImageAttachmentRef, SaveFileAttachment,
    SaveImageAttachment,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use hoh_error::{BoxError, HarnessError};

fn decode_canonical_base64(
    data: &str,
    accept_empty: bool,
    code: AttachmentErrorCode,
) -> Result<Vec<u8>, HarnessError> {
    let decoded = STANDARD.decode(data);
    if let Ok(decoded) = decoded
        && (accept_empty || !data.is_empty())
        && STANDARD.encode(&decoded) == data
    {
        return Ok(decoded);
    }
    Err(AttachmentError::new(
        if code == AttachmentErrorCode::InvalidImageBase64 {
            "Image upload is not canonical base64."
        } else {
            "File upload is not canonical base64."
        },
        code,
    ))
}

/// Decode every image before invoking the store once, including an empty batch.
pub async fn admit_encoded_images<S: AttachmentStore + ?Sized>(
    attachments: &S,
    images: &[EncodedImageAttachment],
) -> Result<Vec<ImageAttachmentRef>, BoxError> {
    let inputs = images
        .iter()
        .map(|image| {
            Ok(SaveImageAttachment {
                data: decode_canonical_base64(
                    &image.data,
                    false,
                    AttachmentErrorCode::InvalidImageBase64,
                )?,
                media_type: image.media_type,
                name: image.name.clone(),
            })
        })
        .collect::<Result<Vec<_>, HarnessError>>()?;
    attachments.save_images(&inputs).await
}

/// Decode one canonical file payload; an empty file is valid.
pub async fn admit_encoded_file<S: AttachmentStore + ?Sized>(
    attachments: &S,
    file: &EncodedFileAttachment,
) -> Result<FileAttachmentRef, BoxError> {
    attachments
        .save_file(&SaveFileAttachment {
            data: decode_canonical_base64(
                &file.data,
                true,
                AttachmentErrorCode::InvalidFileBase64,
            )?,
            name: file.name.clone(),
        })
        .await
}
