//! Machine-routable failures of the subagent seam.

use hoh_error::{BoxError, HarnessError};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubagentErrorCode {
    UnsupportedCapability,
    DuplicateProvider,
    NoProvider,
    ProviderChanged,
    PresetUnavailable,
    PresetSelectionDisabled,
    PresetNotAllowed,
    ModelSelectionDisabled,
    ModelRouteNotAllowed,
    LlmProviderNotFound,
    ModelDoesNotSupportImages,
    DepthExceeded,
    InvalidDepth,
    InvalidCwd,
    WorkingDirectoryUnavailable,
    Unauthorized,
    ActivationLimitReached,
    ActivationClosing,
    ActivationTeardownFailed,
    DuplicateChild,
    Draining,
    InputClosed,
    StructuredOutputPendingInput,
    Cancelled,
    NotResumable,
    NotContinuable,
    ContinuationUnavailable,
    PersistenceUnavailable,
    InvalidAgentName,
    DuplicateAgentName,
    AgentNameNotFound,
    SubagentControlQueryUnavailable,
    SubagentControlProjectionsUnavailable,
    SubagentControlSessionStoreUnavailable,
}

impl SubagentErrorCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UnsupportedCapability => "UNSUPPORTED_CAPABILITY",
            Self::DuplicateProvider => "DUPLICATE_PROVIDER",
            Self::NoProvider => "NO_PROVIDER",
            Self::ProviderChanged => "PROVIDER_CHANGED",
            Self::PresetUnavailable => "PRESET_UNAVAILABLE",
            Self::PresetSelectionDisabled => "PRESET_SELECTION_DISABLED",
            Self::PresetNotAllowed => "PRESET_NOT_ALLOWED",
            Self::ModelSelectionDisabled => "MODEL_SELECTION_DISABLED",
            Self::ModelRouteNotAllowed => "MODEL_ROUTE_NOT_ALLOWED",
            Self::LlmProviderNotFound => "LLM_PROVIDER_NOT_FOUND",
            Self::ModelDoesNotSupportImages => "MODEL_DOES_NOT_SUPPORT_IMAGES",
            Self::DepthExceeded => "DEPTH_EXCEEDED",
            Self::InvalidDepth => "INVALID_DEPTH",
            Self::InvalidCwd => "INVALID_CWD",
            Self::WorkingDirectoryUnavailable => "WORKING_DIRECTORY_UNAVAILABLE",
            Self::Unauthorized => "UNAUTHORIZED",
            Self::ActivationLimitReached => "ACTIVATION_LIMIT_REACHED",
            Self::ActivationClosing => "ACTIVATION_CLOSING",
            Self::ActivationTeardownFailed => "ACTIVATION_TEARDOWN_FAILED",
            Self::DuplicateChild => "DUPLICATE_CHILD",
            Self::Draining => "DRAINING",
            Self::InputClosed => "INPUT_CLOSED",
            Self::StructuredOutputPendingInput => "STRUCTURED_OUTPUT_PENDING_INPUT",
            Self::Cancelled => "CANCELLED",
            Self::NotResumable => "NOT_RESUMABLE",
            Self::NotContinuable => "NOT_CONTINUABLE",
            Self::ContinuationUnavailable => "CONTINUATION_UNAVAILABLE",
            Self::PersistenceUnavailable => "PERSISTENCE_UNAVAILABLE",
            Self::InvalidAgentName => "INVALID_AGENT_NAME",
            Self::DuplicateAgentName => "DUPLICATE_AGENT_NAME",
            Self::AgentNameNotFound => "AGENT_NAME_NOT_FOUND",
            Self::SubagentControlQueryUnavailable => "SUBAGENT_CONTROL_QUERY_UNAVAILABLE",
            Self::SubagentControlProjectionsUnavailable => {
                "SUBAGENT_CONTROL_PROJECTIONS_UNAVAILABLE"
            }
            Self::SubagentControlSessionStoreUnavailable => {
                "SUBAGENT_CONTROL_SESSION_STORE_UNAVAILABLE"
            }
        }
    }
}

pub struct SubagentError;

impl SubagentError {
    pub const NAME: &'static str = "SubagentError";

    pub fn error(message: impl Into<String>, code: SubagentErrorCode) -> HarnessError {
        HarnessError::named(Self::NAME, message, code.as_str())
    }

    pub fn with_cause(
        message: impl Into<String>,
        code: SubagentErrorCode,
        cause: impl Into<BoxError>,
    ) -> HarnessError {
        Self::error(message, code).with_cause(cause)
    }
}
