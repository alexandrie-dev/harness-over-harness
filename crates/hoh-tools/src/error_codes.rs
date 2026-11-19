//! Error identities the registry records on failed tool results. The values are
//! persisted contract; the entry does not re-export them, and consumers compare
//! the recorded strings.

/// Error name the registry records when approval does not grant a call.
pub const APPROVAL_ERROR_NAME: &str = "ApprovalError";

/// Approval code for a call the user rejected.
pub const APPROVAL_REJECTED: &str = "APPROVAL_REJECTED";

/// Approval code for an approval request cancelled without a caller cancellation.
pub const APPROVAL_CANCELLED: &str = "APPROVAL_CANCELLED";

/// Approval code for a call that needs approval when no approval service, Agent, or channel can answer it.
pub const APPROVAL_UNAVAILABLE: &str = "APPROVAL_UNAVAILABLE";

/// Code of `ToolContextUnavailableError`.
pub const TOOL_CONTEXT_UNAVAILABLE: &str = "TOOL_CONTEXT_UNAVAILABLE";
