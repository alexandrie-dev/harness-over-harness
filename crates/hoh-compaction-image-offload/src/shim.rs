//! SHIM(hoh-compaction): the `compaction/summary-error` event that `@deepseek-ai/dsh-compaction`
//! declares (`Events['compaction/summary-error']`, mode waterfall, result boolean). The package
//! `hoh-compaction` is not ported yet; replace this module with its event when it is accepted.

use hoh_error::SharedError;
use hoh_kernel::Waterfall;
use hoh_session::{Session, SessionSeq};
use hoh_timeout::AbortSignal;

/// Payload of `compaction/summary-error`. Recover a failed summary request by synchronously
/// recording a durable change to its selected input; the result is `true` only after making
/// progress, and a listener calls `next` when the failure cannot be recovered.
pub struct CompactionSummaryError {
    /// Session containing the selected input.
    pub session: Session,
    /// Selected message events in request order.
    pub source_event_seqs: Vec<SessionSeq>,
    /// Failure thrown by the summarizer.
    pub error: SharedError,
    /// Optional compaction cancellation signal.
    pub signal: Option<AbortSignal>,
}
hoh_kernel::event!(
    CompactionSummaryError = "compaction/summary-error",
    Waterfall<bool>
);
