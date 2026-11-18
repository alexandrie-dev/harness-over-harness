//! Event-sourced session service. Ordered JSON is the durable authority.
mod events;
pub mod fork;
pub mod known_event_types;
pub mod preparation;
pub mod repair;
pub mod request_header;
pub mod seq_ranges;
mod session;
mod store;
pub mod surface;
pub mod tool_history;
pub mod types;
mod validation;
pub use events::*;
pub use fork::build_fork_seed;
use hoh_error::BoxError;
pub use known_event_types::KNOWN_SESSION_EVENT_TYPES;
pub use preparation::{SessionPreparation, SessionPreparationOptions};
pub use repair::{
    TOOL_NOT_STARTED, TOOL_OUTCOME_UNKNOWN, ToolCallRecovery, interrupted_turn_closers,
};
pub use request_header::{canonical_header, fold_request_header, header_equals};
pub use seq_ranges::{decode_seq_ranges, encode_seq_ranges};
pub use session::{Session, append_plugin_record, plugin_record_of};
use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{Mutex, MutexGuard},
};
pub use store::*;
pub use surface::{
    MessageProjections, SessionMessageProjection, SessionMessageProjectionContext, SessionSurface,
    SurfaceFoldReplacement, SurfaceFoldResult, derive_event_message, fold_surface,
    is_append_surface_event, is_replacement_surface_event, is_surface_eligible_type,
    is_surface_event,
};
pub use types::*;
pub use validation::{adopt_session_event, snapshot_session_event};
pub type Result<T> = std::result::Result<T, BoxError>;
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("{0}")]
    Message(String),
    #[error("{0}")]
    TypeError(String),
}
impl SessionError {
    pub(crate) fn message(text: impl Into<String>) -> BoxError {
        Box::new(Self::Message(text.into()))
    }
    pub(crate) fn type_error(text: impl Into<String>) -> BoxError {
        Box::new(Self::TypeError(text.into()))
    }
    pub fn name(&self) -> &'static str {
        match self {
            Self::Message(_) => "Error",
            Self::TypeError(_) => "TypeError",
        }
    }
}
pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}
pub(crate) fn now() -> i64 {
    // Epoch milliseconds fit this signed representation throughout the process lifetime.
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
pub(crate) fn panic_error(p: Box<dyn std::any::Any + Send>) -> BoxError {
    let s = p
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| p.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "event listener panicked".into());
    SessionError::message(s)
}
pub(crate) fn capture<T>(f: impl FnOnce() -> T) -> Result<T> {
    catch_unwind(AssertUnwindSafe(f)).map_err(panic_error)
}
