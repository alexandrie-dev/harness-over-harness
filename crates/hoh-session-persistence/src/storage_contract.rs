//! Backend-shared validation without migrations or storage machinery.
use crate::errors::*;
use hoh_error::BoxError;
use hoh_session::{
    KNOWN_SESSION_EVENT_TYPES, MAX_SAFE_INTEGER, SESSION_FORMAT_VERSION, SessionError,
    SessionEvent, SessionHeader, SessionId, adopt_session_event,
};
use hoh_util_values::{js_number_string, js_string, snapshot_json_value};
use serde_json::Value;
use std::borrow::Borrow;
use std::sync::Arc;

fn unsupported(reason: String, location: Option<&Arc<SessionLocation>>) -> BoxError {
    let message = match location {
        Some(location) => format!("{reason} (raw log: {})", location.path),
        None => reason,
    };
    Box::new(SessionFormatUnsupportedError::new(
        message,
        location.cloned(),
    ))
}
fn field(value: Option<&Value>) -> String {
    value.map_or_else(|| "undefined".into(), js_string)
}
pub fn assert_stored_id(id: &SessionId, meta: &SessionHeader) -> Result<(), BoxError> {
    if *id != meta.id {
        return Err(Box::new(SessionError::Message(format!(
            "stored session identity mismatch: requested \"{id}\", header contains \"{}\"",
            meta.id
        ))));
    }
    Ok(())
}
pub fn assert_version(
    meta: &SessionHeader,
    location: Option<&Arc<SessionLocation>>,
) -> Result<(), BoxError> {
    if meta.version != SESSION_FORMAT_VERSION {
        return Err(unsupported(
            session_format_version_refusal(meta.id.as_str(), meta.version),
            location,
        ));
    }
    Ok(())
}
pub fn validate_stored_events<'a>(
    meta: &SessionHeader,
    events: &'a mut [SessionEvent],
    location: Option<&Arc<SessionLocation>>,
) -> Result<&'a mut [SessionEvent], BoxError> {
    for event in events.iter() {
        if !KNOWN_SESSION_EVENT_TYPES.contains(&event.event_type()) && !event.ignorable() {
            return Err(unsupported(
                format!(
                    "session \"{}\" contains event type \"{}\" (seq {}) unknown to this harness and not marked ignorable; refusing to interpret the log — it was likely written by a newer harness",
                    meta.id,
                    field(event.as_value().get("type")),
                    field(event.as_value().get("seq"))
                ),
                location,
            ));
        }
        if event.event_type() == "request/header"
            && event.data().get("reason").and_then(Value::as_str) == Some("fallback")
        {
            return Err(unsupported(
                format!(
                    "session \"{}\" contains a request/header event (seq {}) with the unsupported legacy reason \"fallback\"; refusing to interpret the log — it was written by a retired pre-release harness",
                    meta.id,
                    field(event.as_value().get("seq"))
                ),
                location,
            ));
        }
    }
    for event in events.iter() {
        // SessionEvent already exposes an immutable graph; adoption validates it.
        if let Err(cause) = adopt_session_event(event.clone()) {
            if cause.is::<SessionFormatUnsupportedError>() {
                return Err(cause);
            }
            let name = cause
                .downcast_ref::<SessionError>()
                .map_or("Error", SessionError::name);
            return Err(Box::new(SessionPersistenceCorruptionError::new(
                format!(
                    "stored session \"{}\" failed validation: {name}: {cause}",
                    meta.id
                ),
                cause,
            )));
        }
    }
    Ok(events)
}
pub fn materialize_create_header(header: &SessionHeader) -> Result<SessionHeader, BoxError> {
    let value = serde_json::to_value(header)?;
    let Some(mut snapshot) = snapshot_json_value(&value) else {
        return Err(Box::new(SessionError::TypeError(
            "session metadata must be losslessly JSON-serializable".into(),
        )));
    };
    // The serialized record is what gets stored: `extra` can override the typed field.
    let Some(created_at) = snapshot
        .get("createdAt")
        .and_then(Value::as_f64)
        .filter(|n| n.fract() == 0. && *n >= 0. && *n <= MAX_SAFE_INTEGER as f64)
    else {
        return Err(Box::new(SessionError::TypeError(
            "session metadata createdAt must be a non-negative safe integer".into(),
        )));
    };
    // The Rust carrier stores an integer, while JavaScript has one Number type.
    // Normalize an integral float only after validating the exact merged snapshot.
    snapshot["createdAt"] = Value::from(created_at as i64);
    Ok(serde_json::from_value(snapshot)?)
}
pub fn materialize_append_batch<E: Borrow<SessionEvent>>(
    events: &[E],
) -> Result<Vec<SessionEvent>, BoxError> {
    events.iter().map(|event| snapshot_json_value(event.borrow().as_value()).map(SessionEvent::from_value).ok_or_else(|| Box::new(SessionError::TypeError("session event batch is not losslessly JSON-serializable because it contains non-JSON-serializable data".into())) as BoxError)).collect()
}
pub fn assert_contiguous(
    id: &SessionId,
    events: &[SessionEvent],
    cursor: u64,
) -> Result<(), BoxError> {
    for (index, event) in events.iter().enumerate() {
        let expected = cursor as f64 + index as f64;
        if event.as_value().get("seq").and_then(Value::as_f64) != Some(expected) {
            return Err(Box::new(SessionError::Message(format!(
                "append seq mismatch for \"{id}\": expected {} at index {index}, got {}",
                js_number_string(expected),
                field(event.as_value().get("seq"))
            ))));
        }
    }
    Ok(())
}
