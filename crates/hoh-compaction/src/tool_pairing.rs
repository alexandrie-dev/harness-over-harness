use crate::shim::balance_snapshot;
use hoh_error::BoxError;
use hoh_session::{Session, SessionSeq};

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
struct ToolPairingError(String);

fn error(message: impl Into<String>) -> BoxError {
    Box::new(ToolPairingError(message.into()))
}

fn cut_balance(session: &Session, seq: SessionSeq, offset: usize) -> Result<bool, BoxError> {
    let (seqs, cuts) = balance_snapshot(session)?;
    let index = seqs
        .iter()
        .position(|candidate| *candidate == seq)
        .ok_or_else(|| error(format!("tool-pairing balance: surface seq {seq} not found")))?;
    cuts.get(index + offset)
        .copied()
        .ok_or_else(|| error(format!("tool-pairing balance: surface seq {seq} not found")))
}

/// Whether the cut immediately before a current surface sequence is tool-pairing balanced.
pub fn tool_pairing_balanced_before(session: &Session, seq: SessionSeq) -> Result<bool, BoxError> {
    cut_balance(session, seq, 0)
}

/// Whether the cut immediately after a current surface sequence is tool-pairing balanced.
pub fn tool_pairing_balanced_after(session: &Session, seq: SessionSeq) -> Result<bool, BoxError> {
    cut_balance(session, seq, 1)
}
