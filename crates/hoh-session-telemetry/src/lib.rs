//! Capture-side session telemetry contract and coordinator.

mod coordinator;
mod redaction;

pub use coordinator::{
    SessionTelemetryCapture, SessionTelemetryCaptureOptions, SessionTelemetryCoordinator,
};
pub use redaction::{RedactionNext, SessionTelemetryRedactions};

use futures_util::future::BoxFuture;
use hoh_error::BoxError;
use hoh_session::SessionId;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Pre-mapped alerting severity of one outbound record.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionTelemetrySeverity {
    Info,
    Warn,
    Error,
}

/// Whether a record mirrors the ledger or reports a process-local operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionTelemetryChannel {
    Ledger,
    Ops,
}

/// One minimal string-or-number record attribute.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SessionTelemetryAttribute {
    String(String),
    Integer(i64),
    Unsigned(u64),
    Number(f64),
}

impl From<String> for SessionTelemetryAttribute {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}
impl From<&str> for SessionTelemetryAttribute {
    fn from(value: &str) -> Self {
        Self::String(value.to_owned())
    }
}
impl From<u64> for SessionTelemetryAttribute {
    fn from(value: u64) -> Self {
        Self::Unsigned(value)
    }
}
impl From<i64> for SessionTelemetryAttribute {
    fn from(value: i64) -> Self {
        Self::Integer(value)
    }
}

/// Canonical envelope identity, with the event body carried separately.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionTelemetrySourceEvent {
    pub session_id: SessionId,
    pub envelope: Value,
}

/// One logical record handed synchronously to a telemetry backend.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionTelemetryRecord {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_event: Option<SessionTelemetrySourceEvent>,
    pub channel: SessionTelemetryChannel,
    pub time: i64,
    pub severity: SessionTelemetrySeverity,
    pub attributes: IndexMap<String, SessionTelemetryAttribute>,
    pub body: Value,
}

/// Minimum non-blocking backend contract used by the coordinator.
pub trait SessionTelemetrySink: Send + Sync + 'static {
    fn emit(&self, record: SessionTelemetryRecord) -> Result<(), BoxError>;

    fn flush(&self) -> Result<(), BoxError> {
        Ok(())
    }

    fn shutdown(&self) -> BoxFuture<'static, Result<(), BoxError>>;
}

/// Deployment-selected sharing mode, independent of delivery status.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SessionTelemetrySharingStatus {
    Full,
    FeedbackOnly,
    Disabled,
}

/// Loadable backend service surface.
pub trait SessionTelemetryBackendApi: SessionTelemetrySink {
    fn sharing(&self) -> SessionTelemetrySharingStatus;
}

hoh_kernel::service!(pub SessionTelemetry: dyn SessionTelemetryBackendApi = "sessionTelemetry");
