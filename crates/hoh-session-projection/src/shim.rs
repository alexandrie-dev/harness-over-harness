//! Bridges for the current dependency interface, removable at integration.
use hoh_session::Session;
use serde_json::Value;
use std::sync::{Arc, Weak};

// SHIM(hoh-session): Session has exact equality but no public weak identity;
// replace when it provides WeakSession or an equivalent lifecycle token.
// Each current Session owns one stable, distinct immutable header Arc.
pub(crate) fn session_identity(session: &Session) -> (usize, Weak<Value>) {
    let header = session.header();
    (Arc::as_ptr(&header) as usize, Arc::downgrade(&header))
}
