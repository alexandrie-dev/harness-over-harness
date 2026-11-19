//! Typed `read_image` failures.

use hoh_error::HarnessError;

/// Every `read_image` failure the tool classifies itself, before the attachment store sees the
/// bytes:
///
/// - `MODEL_DOES_NOT_SUPPORT_IMAGES`: the calling route does not declare image input; switch to an
///   image-capable model.
/// - `IMAGE_UNSUPPORTED_FORMAT`: the extension, the file signature, or the deployment's accepted
///   media types exclude the format; convert the file to an accepted format or read another file.
///
/// Failures the attachment store classifies keep their `AttachmentError` codes.
pub const IMAGE_INPUT_ERROR_CODES: [&str; 2] =
    ["MODEL_DOES_NOT_SUPPORT_IMAGES", "IMAGE_UNSUPPORTED_FORMAT"];

/// One `read_image` failure code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageInputErrorCode {
    ModelDoesNotSupportImages,
    UnsupportedFormat,
}

impl ImageInputErrorCode {
    /// The stable wire code.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ModelDoesNotSupportImages => IMAGE_INPUT_ERROR_CODES[0],
            Self::UnsupportedFormat => IMAGE_INPUT_ERROR_CODES[1],
        }
    }
}

/// Typed `read_image` failure; `code` is one of [`IMAGE_INPUT_ERROR_CODES`].
pub struct ImageInputError;

impl ImageInputError {
    /// The error name carried by [`HarnessError`].
    pub const NAME: &str = "ImageInputError";

    /// A model-facing refusal naming the path and the safe next step.
    #[allow(clippy::new_ret_no_self)]
    pub fn new(message: impl Into<String>, code: ImageInputErrorCode) -> HarnessError {
        HarnessError::named(Self::NAME, message, code.as_str())
    }
}
