use hoh_error::BoxError;

/// Classified failures shared by conversion and its consumers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OfficeKitErrorCode {
    InputTooLarge,
    OutputTooLarge,
    InvalidDocument,
    UnsupportedFormat,
    InvalidOutput,
    Timeout,
    Unavailable,
    Failed,
    Busy,
    SourceChanged,
    InvalidRequest,
}

impl OfficeKitErrorCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InputTooLarge => "input-too-large",
            Self::OutputTooLarge => "output-too-large",
            Self::InvalidDocument => "invalid-document",
            Self::UnsupportedFormat => "unsupported-format",
            Self::InvalidOutput => "invalid-output",
            Self::Timeout => "timeout",
            Self::Unavailable => "unavailable",
            Self::Failed => "failed",
            Self::Busy => "busy",
            Self::SourceChanged => "source-changed",
            Self::InvalidRequest => "invalid-request",
        }
    }

    pub fn from_code(value: &str) -> Option<Self> {
        Some(match value {
            "input-too-large" => Self::InputTooLarge,
            "output-too-large" => Self::OutputTooLarge,
            "invalid-document" => Self::InvalidDocument,
            "unsupported-format" => Self::UnsupportedFormat,
            "invalid-output" => Self::InvalidOutput,
            "timeout" => Self::Timeout,
            "unavailable" => Self::Unavailable,
            "failed" => Self::Failed,
            "busy" => Self::Busy,
            "source-changed" => Self::SourceChanged,
            "invalid-request" => Self::InvalidRequest,
            _ => return None,
        })
    }
}

impl serde::Serialize for OfficeKitErrorCode {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> serde::Deserialize<'de> for OfficeKitErrorCode {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = <String as serde::Deserialize>::deserialize(deserializer)?;
        Self::from_code(&value).ok_or_else(|| {
            serde::de::Error::custom(format!("unknown OfficeKit error code: {value}"))
        })
    }
}

impl schemars::JsonSchema for OfficeKitErrorCode {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "OfficeKitErrorCode".into()
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        <String as schemars::JsonSchema>::json_schema(generator)
    }
}

/// Engine details remain in `source`; the lowercase code is a wire value.
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct OfficeKitError {
    pub code: OfficeKitErrorCode,
    pub message: String,
    #[source]
    pub source: Option<BoxError>,
}

impl OfficeKitError {
    pub fn new(code: OfficeKitErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            source: None,
        }
    }

    pub fn with_source(
        code: OfficeKitErrorCode,
        message: impl Into<String>,
        source: impl Into<BoxError>,
    ) -> Self {
        Self {
            code,
            message: message.into(),
            source: Some(source.into()),
        }
    }

    pub fn name(&self) -> &'static str {
        "OfficeKitError"
    }
}

pub type Result<T> = std::result::Result<T, OfficeKitError>;
