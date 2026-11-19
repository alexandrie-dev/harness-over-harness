//! Private dependency bridge used until `hoh-session` exposes weak lifecycle-owned extension state.

use hoh_error::BoxError;
use hoh_session::{Session, SessionEvent, SessionSeq};

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
struct ToolPairingError(String);

fn error(message: impl Into<String>) -> BoxError {
    Box::new(ToolPairingError(message.into()))
}

fn event_delta(event: &SessionEvent) -> i64 {
    match event.event_type() {
        "assistant/message" => event
            .data()
            .get("message")
            .and_then(|message| message.get("content"))
            .and_then(serde_json::Value::as_array)
            .map_or(0, |content| {
                content
                    .iter()
                    .filter(|block| {
                        block.get("type").and_then(serde_json::Value::as_str) == Some("tool-call")
                    })
                    .count() as i64
            }),
        "tool/result" => -1,
        _ => 0,
    }
}

// SHIM(hoh-session): weak lifecycle identity plus a plugin-owned extension slot for BalanceCache;
// replace this full-surface fallback when hoh-session provides that state without a global registry.
pub(crate) fn balance_snapshot(
    session: &Session,
) -> Result<(Vec<SessionSeq>, Vec<bool>), BoxError> {
    let seqs = session.surface()?.nodes()?;
    let mut cuts = Vec::with_capacity(seqs.len() + 1);
    cuts.push(true);
    let mut in_progress = 0_i64;
    for &seq in &seqs {
        let event = session.event_at(seq).ok_or_else(|| {
            error(format!(
                "tool-pairing balance: surface seq {seq} has no matching session event (corrupt surface)"
            ))
        })?;
        if event.seq() != seq {
            return Err(error(format!(
                "tool-pairing balance: surface seq {seq} has no matching session event (corrupt surface)"
            )));
        }
        in_progress += event_delta(&event);
        if in_progress < 0 {
            return Err(error(format!(
                "tool-pairing balance: tool/result at surface seq {seq} has no matching tool-call (corrupt surface)"
            )));
        }
        cuts.push(in_progress == 0);
    }
    Ok((seqs, cuts))
}
