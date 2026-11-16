//! Stable attachment failure codes for protocol error routing.

use hoh_error::HarnessError;
use serde::{Deserialize, Serialize};
use std::{error::Error, fmt};

macro_rules! codes {
    ($name:ident { $($variant:ident => $code:literal),+ $(,)? }) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
        pub enum $name {
            $(#[serde(rename = $code)] $variant),+
        }

        impl $name {
            pub const fn as_str(self) -> &'static str {
                match self { $(Self::$variant => $code),+ }
            }

            pub fn from_code(code: &str) -> Option<Self> {
                match code { $($code => Some(Self::$variant),)+ _ => None }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

codes!(ImageAdmissionErrorCode {
    TooManyImages => "TOO_MANY_IMAGES",
    ImagesTooLarge => "IMAGES_TOO_LARGE",
    UnsupportedImageType => "UNSUPPORTED_IMAGE_TYPE",
    InvalidImageBase64 => "INVALID_IMAGE_BASE64",
    InvalidImage => "INVALID_IMAGE",
    ImageTypeMismatch => "IMAGE_TYPE_MISMATCH",
    ImageTooLarge => "IMAGE_TOO_LARGE",
    ImageTooManyPixels => "IMAGE_TOO_MANY_PIXELS",
    ImageDimensionTooLarge => "IMAGE_DIMENSION_TOO_LARGE",
});

codes!(AttachmentErrorCode {
    TooManyImages => "TOO_MANY_IMAGES",
    ImagesTooLarge => "IMAGES_TOO_LARGE",
    UnsupportedImageType => "UNSUPPORTED_IMAGE_TYPE",
    InvalidImageBase64 => "INVALID_IMAGE_BASE64",
    InvalidImage => "INVALID_IMAGE",
    ImageTypeMismatch => "IMAGE_TYPE_MISMATCH",
    ImageTooLarge => "IMAGE_TOO_LARGE",
    ImageTooManyPixels => "IMAGE_TOO_MANY_PIXELS",
    ImageDimensionTooLarge => "IMAGE_DIMENSION_TOO_LARGE",
    InvalidFileBase64 => "INVALID_FILE_BASE64",
    InvalidAttachmentRef => "INVALID_ATTACHMENT_REF",
    AttachmentCorrupt => "ATTACHMENT_CORRUPT",
    AttachmentWriteFailed => "ATTACHMENT_WRITE_FAILED",
    AttachmentNotFound => "ATTACHMENT_NOT_FOUND",
    AttachmentReadFailed => "ATTACHMENT_READ_FAILED",
    AttachmentProjectionUnsupported => "ATTACHMENT_PROJECTION_UNSUPPORTED",
    AttachmentFilesUnsupported => "ATTACHMENT_FILES_UNSUPPORTED",
});

impl From<ImageAdmissionErrorCode> for AttachmentErrorCode {
    fn from(code: ImageAdmissionErrorCode) -> Self {
        match code {
            ImageAdmissionErrorCode::TooManyImages => Self::TooManyImages,
            ImageAdmissionErrorCode::ImagesTooLarge => Self::ImagesTooLarge,
            ImageAdmissionErrorCode::UnsupportedImageType => Self::UnsupportedImageType,
            ImageAdmissionErrorCode::InvalidImageBase64 => Self::InvalidImageBase64,
            ImageAdmissionErrorCode::InvalidImage => Self::InvalidImage,
            ImageAdmissionErrorCode::ImageTypeMismatch => Self::ImageTypeMismatch,
            ImageAdmissionErrorCode::ImageTooLarge => Self::ImageTooLarge,
            ImageAdmissionErrorCode::ImageTooManyPixels => Self::ImageTooManyPixels,
            ImageAdmissionErrorCode::ImageDimensionTooLarge => Self::ImageDimensionTooLarge,
        }
    }
}

/// Constructor namespace; every coded error remains a `HarnessError`.
pub struct AttachmentError;

impl AttachmentError {
    pub const NAME: &'static str = "AttachmentError";

    #[allow(clippy::new_ret_no_self)]
    pub fn new(message: impl Into<String>, code: impl Into<AttachmentErrorCode>) -> HarnessError {
        HarnessError::named(Self::NAME, message, code.into().as_str())
    }
}

/// Recognize by code, including a differently named `HarnessError` or shared reason.
pub fn is_attachment_error(error: &(dyn Error + 'static)) -> bool {
    HarnessError::of(error)
        .is_some_and(|error| AttachmentErrorCode::from_code(error.code()).is_some())
}

/// Distinguish caller-correctable image input from storage failures.
pub fn is_image_admission_error(error: &(dyn Error + 'static)) -> bool {
    HarnessError::of(error)
        .is_some_and(|error| ImageAdmissionErrorCode::from_code(error.code()).is_some())
}
