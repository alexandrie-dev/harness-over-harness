//! Durable references, wire uploads, and provider payload vocabulary.

pub use crate::brand::{AttachmentId, ImageVariantId};
use crate::request_projection::ProjectedDimensions;
use futures_util::stream::BoxStream;
use hoh_error::BoxError;
use hoh_timeout::AbortSignal;
use serde::{Deserialize, Deserializer, Serialize};
use std::fmt;

/// Raster formats accepted by the version-one image path.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImageMediaType {
    #[serde(rename = "image/png")]
    Png,
    #[serde(rename = "image/jpeg")]
    Jpeg,
    #[serde(rename = "image/webp")]
    Webp,
    #[serde(rename = "image/gif")]
    Gif,
}

impl ImageMediaType {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Webp => "image/webp",
            Self::Gif => "image/gif",
        }
    }
}

impl fmt::Display for ImageMediaType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageAttachmentRef {
    pub attachment_id: AttachmentId,
    pub media_type: ImageMediaType,
    pub bytes: u64,
    pub width: u64,
    pub height: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_dimensions: Option<ProjectedDimensions>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileAttachmentRef {
    pub attachment_id: AttachmentId,
    pub name: String,
    pub bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EncodedFileAttachment {
    pub data: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SaveFileAttachment {
    pub data: Vec<u8>,
    pub name: Option<String>,
}

/// Commit exact file bytes from ordered, bounded chunks without collecting the file.
pub struct SaveFileStreamAttachment<'a> {
    /// Source reads are driven by the provider to preserve backpressure.
    pub data: BoxStream<'a, Result<Vec<u8>, BoxError>>,
    /// Optional cancellation for source reads and storage writes.
    pub signal: Option<AbortSignal>,
    /// Display name only; never interpreted as a path.
    pub name: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageAttachmentLimits {
    pub max_image_bytes: u64,
    pub max_images_per_message: u64,
    pub max_message_image_bytes: u64,
    pub max_image_pixels: u64,
    pub max_image_dimension: u64,
    pub media_types: Vec<ImageMediaType>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EncodedImageAttachment {
    pub media_type: ImageMediaType,
    pub data: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum PromptContentPart {
    Text {
        text: String,
    },
    Image {
        media_type: ImageMediaType,
        data: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
}

/// Host prompt content after resolving upload receipts, before image admission.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum AttachmentAdmissionPart {
    Text {
        text: String,
    },
    #[serde(rename = "image")]
    EncodedImage {
        media_type: ImageMediaType,
        data: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
    File {
        attachment: FileAttachmentRef,
    },
    #[serde(rename = "image")]
    StoredImage {
        attachment: ImageAttachmentRef,
        encoded_bytes: u64,
    },
}

impl<'de> Deserialize<'de> for AttachmentAdmissionPart {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // Two image forms share the same wire tag and differ by their payload.
        #[derive(Deserialize)]
        #[serde(tag = "type", rename_all = "camelCase")]
        enum Part {
            Text {
                text: String,
            },
            Image {
                #[serde(flatten)]
                payload: Image,
            },
            File {
                attachment: FileAttachmentRef,
            },
        }

        #[derive(Deserialize)]
        #[serde(untagged, rename_all_fields = "camelCase")]
        enum Image {
            Stored {
                attachment: ImageAttachmentRef,
                encoded_bytes: u64,
            },
            Encoded {
                media_type: ImageMediaType,
                data: String,
                #[serde(default)]
                name: Option<String>,
            },
        }

        Ok(match Part::deserialize(deserializer)? {
            Part::Text { text } => Self::Text { text },
            Part::File { attachment } => Self::File { attachment },
            Part::Image {
                payload:
                    Image::Stored {
                        attachment,
                        encoded_bytes,
                    },
            } => Self::StoredImage {
                attachment,
                encoded_bytes,
            },
            Part::Image {
                payload:
                    Image::Encoded {
                        media_type,
                        data,
                        name,
                    },
            } => Self::EncodedImage {
                media_type,
                data,
                name,
            },
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum AdmittedPromptContentPart {
    Text { text: String },
    Image { attachment: ImageAttachmentRef },
    File { attachment: FileAttachmentRef },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SaveImageAttachment {
    pub data: Vec<u8>,
    pub media_type: ImageMediaType,
    pub name: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredImageAttachment {
    pub r#ref: ImageAttachmentRef,
    pub data: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageRequestTarget {
    pub width: u64,
    pub height: u64,
    pub max_bytes: u64,
}

/// Provider-compatible depth, fixed by the source contract.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RequestImageDepth {
    #[serde(rename = "uchar")]
    Uchar,
}

/// Provider-compatible color space, fixed by the source contract.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RequestImageSpace {
    #[serde(rename = "srgb")]
    Srgb,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequestImageAttachment {
    pub variant_id: ImageVariantId,
    pub attachment: ImageAttachmentRef,
    pub data: Vec<u8>,
    pub media_type: ImageMediaType,
    pub bytes: u64,
    pub width: u64,
    pub height: u64,
    pub depth: RequestImageDepth,
    pub space: RequestImageSpace,
    pub has_alpha: bool,
}
