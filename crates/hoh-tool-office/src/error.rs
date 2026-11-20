use hoh_error::{BoxError, HarnessError};
use hoh_office_kit::OfficeKitErrorCode;
use serde::{Deserialize, Serialize};

/// Stable failures classified by the Office tool package.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OfficeToolErrorCode {
    #[serde(rename = "OFFICE_UNAVAILABLE")]
    Unavailable,
    #[serde(rename = "OFFICE_BUSY")]
    Busy,
    #[serde(rename = "OFFICE_INPUT_TOO_LARGE")]
    InputTooLarge,
    #[serde(rename = "OFFICE_OUTPUT_TOO_LARGE")]
    OutputTooLarge,
    #[serde(rename = "OFFICE_INVALID_DOCUMENT")]
    InvalidDocument,
    #[serde(rename = "OFFICE_UNSUPPORTED_FORMAT")]
    UnsupportedFormat,
    #[serde(rename = "OFFICE_INVALID_REQUEST")]
    InvalidRequest,
    #[serde(rename = "OFFICE_INVALID_OUTPUT")]
    InvalidOutput,
    #[serde(rename = "OFFICE_TIMEOUT")]
    Timeout,
    #[serde(rename = "OFFICE_SOURCE_CHANGED")]
    SourceChanged,
    #[serde(rename = "OFFICE_CONVERSION_FAILED")]
    ConversionFailed,
    #[serde(rename = "OFFICE_UNSUPPORTED_REMOTE_WORKSPACE")]
    UnsupportedRemoteWorkspace,
    #[serde(rename = "MODEL_DOES_NOT_SUPPORT_IMAGES")]
    ModelDoesNotSupportImages,
}

impl OfficeToolErrorCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unavailable => "OFFICE_UNAVAILABLE",
            Self::Busy => "OFFICE_BUSY",
            Self::InputTooLarge => "OFFICE_INPUT_TOO_LARGE",
            Self::OutputTooLarge => "OFFICE_OUTPUT_TOO_LARGE",
            Self::InvalidDocument => "OFFICE_INVALID_DOCUMENT",
            Self::UnsupportedFormat => "OFFICE_UNSUPPORTED_FORMAT",
            Self::InvalidRequest => "OFFICE_INVALID_REQUEST",
            Self::InvalidOutput => "OFFICE_INVALID_OUTPUT",
            Self::Timeout => "OFFICE_TIMEOUT",
            Self::SourceChanged => "OFFICE_SOURCE_CHANGED",
            Self::ConversionFailed => "OFFICE_CONVERSION_FAILED",
            Self::UnsupportedRemoteWorkspace => "OFFICE_UNSUPPORTED_REMOTE_WORKSPACE",
            Self::ModelDoesNotSupportImages => "MODEL_DOES_NOT_SUPPORT_IMAGES",
        }
    }
}

impl From<OfficeKitErrorCode> for OfficeToolErrorCode {
    fn from(code: OfficeKitErrorCode) -> Self {
        match code {
            OfficeKitErrorCode::Unavailable => Self::Unavailable,
            OfficeKitErrorCode::Busy => Self::Busy,
            OfficeKitErrorCode::InputTooLarge => Self::InputTooLarge,
            OfficeKitErrorCode::OutputTooLarge => Self::OutputTooLarge,
            OfficeKitErrorCode::InvalidDocument => Self::InvalidDocument,
            OfficeKitErrorCode::UnsupportedFormat => Self::UnsupportedFormat,
            OfficeKitErrorCode::InvalidRequest => Self::InvalidRequest,
            OfficeKitErrorCode::InvalidOutput => Self::InvalidOutput,
            OfficeKitErrorCode::Timeout => Self::Timeout,
            OfficeKitErrorCode::SourceChanged => Self::SourceChanged,
            OfficeKitErrorCode::Failed => Self::ConversionFailed,
        }
    }
}

/// Constructor namespace for typed Office tool failures.
pub struct OfficeToolError;

impl OfficeToolError {
    pub const NAME: &'static str = "OfficeToolError";

    #[allow(clippy::new_ret_no_self)] // Constructor namespace required by PORTING section 7.
    pub fn new(message: impl Into<String>, code: OfficeToolErrorCode) -> HarnessError {
        HarnessError::named(Self::NAME, message, code.as_str())
    }

    pub fn with_cause(
        message: impl Into<String>,
        code: OfficeToolErrorCode,
        cause: impl Into<BoxError>,
    ) -> HarnessError {
        Self::new(message, code).with_cause(cause)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_every_office_kit_failure_code() {
        let cases = [
            (
                OfficeKitErrorCode::Unavailable,
                OfficeToolErrorCode::Unavailable,
            ),
            (OfficeKitErrorCode::Busy, OfficeToolErrorCode::Busy),
            (
                OfficeKitErrorCode::InputTooLarge,
                OfficeToolErrorCode::InputTooLarge,
            ),
            (
                OfficeKitErrorCode::OutputTooLarge,
                OfficeToolErrorCode::OutputTooLarge,
            ),
            (
                OfficeKitErrorCode::InvalidDocument,
                OfficeToolErrorCode::InvalidDocument,
            ),
            (
                OfficeKitErrorCode::UnsupportedFormat,
                OfficeToolErrorCode::UnsupportedFormat,
            ),
            (
                OfficeKitErrorCode::InvalidRequest,
                OfficeToolErrorCode::InvalidRequest,
            ),
            (
                OfficeKitErrorCode::InvalidOutput,
                OfficeToolErrorCode::InvalidOutput,
            ),
            (OfficeKitErrorCode::Timeout, OfficeToolErrorCode::Timeout),
            (
                OfficeKitErrorCode::SourceChanged,
                OfficeToolErrorCode::SourceChanged,
            ),
            (
                OfficeKitErrorCode::Failed,
                OfficeToolErrorCode::ConversionFailed,
            ),
        ];
        for (kit, tool) in cases {
            assert_eq!(OfficeToolErrorCode::from(kit), tool);
        }
    }

    #[test]
    fn constructs_named_coded_errors_with_causes() {
        let error = OfficeToolError::with_cause(
            "cannot render",
            OfficeToolErrorCode::InvalidRequest,
            BoxError::from("kit refusal"),
        );
        assert_eq!(error.name(), "OfficeToolError");
        assert_eq!(error.code(), "OFFICE_INVALID_REQUEST");
        assert_eq!(error.cause().unwrap().to_string(), "kit refusal");
    }
}
