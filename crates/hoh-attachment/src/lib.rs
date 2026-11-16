//! Port of @deepseek-ai/dsh-attachment (packages/attachment/attachment).
//! Durable immutable attachment storage seam (`ctx.attachments`).

pub mod admission;
pub mod brand;
pub mod error;
pub mod request_projection;
pub mod types;

pub use admission::{admit_encoded_file, admit_encoded_images};
pub use brand::{AttachmentId, ImageVariantId};
pub use error::{
    AttachmentError, AttachmentErrorCode, ImageAdmissionErrorCode, is_attachment_error,
    is_image_admission_error,
};
pub use request_projection::{ProjectedDimensions, long_edge_dimensions, request_image_dimensions};
pub use types::*;

use futures_util::{future::BoxFuture, stream::BoxStream};
use hoh_error::{BoxError, HarnessError};
use hoh_timeout::AbortSignal;
use std::error::Error;

pub type AttachmentIdType = AttachmentId;

/// Immutable binary attachment service. Providers validate before publishing references.
///
/// Register a provider using `ctx.provide::<Attachments>(Arc::new(provider)).await?`.
/// Async methods use boxed Send futures so different providers share one service key.
pub trait AttachmentStore: Send + Sync + 'static {
    fn image_limits(&self) -> &ImageAttachmentLimits;

    fn validate_image<'a>(
        &'a self,
        input: &'a SaveImageAttachment,
    ) -> BoxFuture<'a, Result<(), BoxError>>;

    /// Batch policy is checked before any per-image validation or writes.
    fn validate_image_batch(&self, inputs: &[SaveImageAttachment]) -> Result<(), HarnessError> {
        let limits = self.image_limits();
        assert_message_images(
            limits,
            inputs.len() as u64,
            inputs.iter().fold(0_u64, |sum, input| {
                sum.saturating_add(input.data.len() as u64)
            }),
        )?;
        for input in inputs {
            if !limits.media_types.contains(&input.media_type) {
                return Err(AttachmentError::new(
                    format!(
                        "Image type {} is not accepted by this deployment.",
                        input.media_type
                    ),
                    AttachmentErrorCode::UnsupportedImageType,
                ));
            }
        }
        Ok(())
    }

    /// Validate the whole batch, then commit each member in input order.
    fn save_images<'a>(
        &'a self,
        inputs: &'a [SaveImageAttachment],
    ) -> BoxFuture<'a, Result<Vec<ImageAttachmentRef>, BoxError>> {
        Box::pin(async move {
            self.validate_image_batch(inputs)?;
            for input in inputs {
                self.validate_image(input).await?;
            }
            let mut refs = Vec::with_capacity(inputs.len());
            for input in inputs {
                refs.push(self.save_image(input).await?);
            }
            Ok(refs)
        })
    }

    /// Admit encoded images while passing text and durable references through in order.
    fn admit_prompt_content<'a>(
        &'a self,
        content: &'a [AttachmentAdmissionPart],
    ) -> BoxFuture<'a, Result<Vec<AdmittedPromptContentPart>, BoxError>> {
        Box::pin(async move {
            let mut encoded = Vec::new();
            let mut stored_count = 0_u64;
            let mut stored_bytes = 0_u64;
            for part in content {
                match part {
                    AttachmentAdmissionPart::EncodedImage {
                        media_type,
                        data,
                        name,
                    } => {
                        encoded.push(EncodedImageAttachment {
                            media_type: *media_type,
                            data: data.clone(),
                            name: name.clone(),
                        });
                    }
                    AttachmentAdmissionPart::StoredImage { encoded_bytes, .. } => {
                        stored_count += 1;
                        stored_bytes = stored_bytes.saturating_add(*encoded_bytes);
                    }
                    AttachmentAdmissionPart::Text { .. } | AttachmentAdmissionPart::File { .. } => {
                    }
                }
            }
            if stored_count > 0 {
                let bytes = encoded.iter().fold(stored_bytes, |sum, image| {
                    sum.saturating_add(base64_byte_length(&image.data))
                });
                assert_message_images(
                    self.image_limits(),
                    encoded.len() as u64 + stored_count,
                    bytes,
                )?;
            }
            let refs = if encoded.is_empty() {
                Vec::new()
            } else {
                admit_encoded_images(self, &encoded).await?
            };
            let reference_count = refs.len();
            let image_count = encoded.len();
            let mut next = refs.into_iter();
            Ok(content
                .iter()
                .map(|part| Ok(match part {
                    AttachmentAdmissionPart::Text { text } => {
                        AdmittedPromptContentPart::Text { text: text.clone() }
                    }
                    AttachmentAdmissionPart::File { attachment } => {
                        AdmittedPromptContentPart::File {
                            attachment: attachment.clone(),
                        }
                    }
                    AttachmentAdmissionPart::StoredImage { attachment, .. } => {
                        AdmittedPromptContentPart::Image {
                            attachment: attachment.clone(),
                        }
                    }
                    AttachmentAdmissionPart::EncodedImage { .. } => {
                        AdmittedPromptContentPart::Image {
                            attachment: next.next().ok_or_else(|| AttachmentError::new(
                                format!("Attachment store returned {reference_count} image references for {image_count} images."),
                                AttachmentErrorCode::AttachmentWriteFailed,
                            ))?,
                        }
                    }
                }))
                .collect::<Result<_, HarnessError>>()?)
        })
    }

    fn admit_encoded_file<'a>(
        &'a self,
        input: &'a EncodedFileAttachment,
    ) -> BoxFuture<'a, Result<FileAttachmentRef, BoxError>> {
        Box::pin(admit_encoded_file(self, input))
    }

    fn is_attachment_error(&self, error: &(dyn Error + 'static)) -> bool {
        is_attachment_error(error)
    }

    fn save_image<'a>(
        &'a self,
        input: &'a SaveImageAttachment,
    ) -> BoxFuture<'a, Result<ImageAttachmentRef, BoxError>>;

    /// Read stored bytes and verify ID, media type, byte length, and dimensions.
    /// Providers preserve name/original_dimensions and propagate the signal's reason.
    fn read_image<'a>(
        &'a self,
        reference: &'a ImageAttachmentRef,
        signal: Option<&'a AbortSignal>,
    ) -> BoxFuture<'a, Result<StoredImageAttachment, BoxError>>;

    fn image_host_path(
        &self,
        _reference: &ImageAttachmentRef,
    ) -> Result<Option<String>, HarnessError> {
        Ok(None)
    }

    fn save_file<'a>(
        &'a self,
        _input: &'a SaveFileAttachment,
    ) -> BoxFuture<'a, Result<FileAttachmentRef, BoxError>> {
        Box::pin(async {
            Err(AttachmentError::new(
                "The mounted attachment provider cannot store verbatim files.",
                AttachmentErrorCode::AttachmentFilesUnsupported,
            )
            .into())
        })
    }

    /// Store ordered file chunks with backpressure, without collecting the full file.
    /// Providers without streamed storage reject without reading the source.
    fn save_file_stream<'a>(
        &'a self,
        _input: SaveFileStreamAttachment<'a>,
    ) -> BoxFuture<'a, Result<FileAttachmentRef, BoxError>> {
        Box::pin(async {
            Err(AttachmentError::new(
                "The mounted attachment provider cannot stream verbatim files.",
                AttachmentErrorCode::AttachmentFilesUnsupported,
            )
            .into())
        })
    }

    /// Read bounded file chunks while verifying the stored digest and length.
    /// The default checks cancellation at the first read, then rejects without a chunk.
    fn read_file_stream<'a>(
        &'a self,
        _reference: &'a FileAttachmentRef,
        signal: Option<&'a AbortSignal>,
    ) -> BoxStream<'a, Result<Vec<u8>, BoxError>> {
        Box::pin(futures_util::stream::once(async move {
            if let Some(signal) = signal {
                signal.throw_if_aborted()?;
            }
            Err(AttachmentError::new(
                "The mounted attachment provider cannot read verbatim files.",
                AttachmentErrorCode::AttachmentFilesUnsupported,
            )
            .into())
        }))
    }

    fn file_host_path(
        &self,
        _reference: &FileAttachmentRef,
    ) -> Result<Option<String>, HarnessError> {
        Ok(None)
    }

    /// Derive or read a deterministic, route-sized model-request image.
    /// The default chooses the signal's reason at call time, before returning a future.
    fn read_image_request<'a>(
        &'a self,
        _reference: &'a ImageAttachmentRef,
        _target: &'a ImageRequestTarget,
        signal: Option<&'a AbortSignal>,
    ) -> BoxFuture<'a, Result<RequestImageAttachment, BoxError>> {
        let error: BoxError = match signal.map(AbortSignal::throw_if_aborted).transpose() {
            Err(reason) => reason.into(),
            Ok(_) => AttachmentError::new(
                "The mounted attachment provider cannot derive model-request images.",
                AttachmentErrorCode::AttachmentProjectionUnsupported,
            )
            .into(),
        };
        Box::pin(async move { Err(error) })
    }
}

hoh_kernel::service!(pub Attachments: dyn AttachmentStore = "attachments");

fn assert_message_images(
    limits: &ImageAttachmentLimits,
    count: u64,
    bytes: u64,
) -> Result<(), HarnessError> {
    if count > limits.max_images_per_message {
        return Err(AttachmentError::new(
            "Image batch exceeds the configured image-count limit.",
            AttachmentErrorCode::TooManyImages,
        ));
    }
    if bytes > limits.max_message_image_bytes {
        return Err(AttachmentError::new(
            "Image batch exceeds the configured aggregate image-byte limit.",
            AttachmentErrorCode::ImagesTooLarge,
        ));
    }
    Ok(())
}

/// Node Buffer.byteLength(data, 'base64') estimates length before canonical decoding.
fn base64_byte_length(data: &str) -> u64 {
    let mut length = data.encode_utf16().count() as u64;
    if data.ends_with('=') {
        length -= 1;
        if data.ends_with("==") {
            length -= 1;
        }
    }
    length.saturating_mul(3) / 4
}
