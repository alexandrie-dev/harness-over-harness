//! The source transport classifier, with explicit Node-style code metadata.
use hoh_error::{HarnessError, SharedError};
use hoh_util_values::js_string;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::error::Error;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransportFailure {
    pub code: String,
    pub message: String,
}

/// Error metadata normally attached by Node/undici. Rust errors do not expose arbitrary properties.
#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("{}", error_text(.name, .message))]
    Node {
        name: String,
        message: String,
        code: Option<Value>,
        #[source]
        cause: Option<SharedError>,
        attempts: Option<Vec<SharedError>>,
    },
}
impl TransportError {
    pub fn new(message: impl Into<String>, code: Option<&str>) -> Self {
        Self::Node {
            name: "Error".into(),
            message: message.into(),
            code: code.map(|code| Value::String(code.into())),
            cause: None,
            attempts: None,
        }
    }
    pub fn fetch_failed(cause: impl Into<hoh_error::BoxError>) -> Self {
        Self::new("fetch failed", None)
            .with_name("TypeError")
            .with_cause(cause)
    }
    pub fn aggregate(message: impl Into<String>, attempts: Vec<SharedError>) -> Self {
        Self::Node {
            name: "AggregateError".into(),
            message: message.into(),
            code: None,
            cause: None,
            attempts: Some(attempts),
        }
    }
    pub fn with_name(mut self, value: impl Into<String>) -> Self {
        let Self::Node { name, .. } = &mut self;
        *name = value.into();
        self
    }
    pub fn with_code(mut self, value: Value) -> Self {
        let Self::Node { code, .. } = &mut self;
        *code = Some(value);
        self
    }
    pub fn with_cause(mut self, value: impl Into<hoh_error::BoxError>) -> Self {
        let Self::Node { cause, .. } = &mut self;
        *cause = Some(SharedError::new(value));
        self
    }
}
fn error_text(name: &str, message: &str) -> String {
    match (name.is_empty(), message.is_empty()) {
        (true, _) => message.into(),
        (false, true) => name.into(),
        (false, false) => format!("{name}: {message}"),
    }
}

/// An explicitly thrown JSON value, including strings, instead of a JavaScript Error instance.
#[derive(Debug, thiserror::Error)]
pub enum TransportValue {
    #[error("{}", js_string(.0))]
    Thrown(Value),
    #[error("undefined")]
    Undefined,
}

fn unshare<'a>(mut error: &'a (dyn Error + 'static)) -> &'a (dyn Error + 'static) {
    while let Some(shared) = error.downcast_ref::<SharedError>() {
        error = shared.get_ref();
    }
    error
}
fn innermost<'a>(error: &'a (dyn Error + 'static)) -> &'a (dyn Error + 'static) {
    let mut error = unshare(error);
    for _ in 0..4 {
        if error.is::<TransportValue>() {
            break;
        }
        let Some(cause) = error.source() else {
            break;
        };
        error = unshare(cause);
    }
    error
}
fn system_code<'a>(error: &'a (dyn Error + 'static)) -> Option<&'a str> {
    if let Some(TransportError::Node { code, .. }) = error.downcast_ref::<TransportError>() {
        return code.as_ref().and_then(Value::as_str);
    }
    HarnessError::of(error).map(HarnessError::code)
}
fn message_of(error: &(dyn Error + 'static), fallback: &(dyn Error + 'static)) -> String {
    let message = error.downcast_ref::<TransportError>().map_or_else(
        || error.to_string(),
        |TransportError::Node { message, .. }| message.clone(),
    );
    if message.is_empty() {
        fallback.to_string()
    } else {
        message
    }
}
fn class(code: Option<&str>) -> Option<usize> {
    match code? {
        "ENOTFOUND" | "EAI_AGAIN" | "EAI_FAIL" | "EAI_NONAME" | "EAI_NODATA" => Some(0),
        "ECONNREFUSED" => Some(1),
        "ETIMEDOUT" | "UND_ERR_CONNECT_TIMEOUT" => Some(2),
        "CERT_HAS_EXPIRED"
        | "CERT_NOT_YET_VALID"
        | "CERT_REVOKED"
        | "CERT_UNTRUSTED"
        | "CERT_REJECTED"
        | "CERT_SIGNATURE_FAILURE"
        | "DEPTH_ZERO_SELF_SIGNED_CERT"
        | "SELF_SIGNED_CERT_IN_CHAIN"
        | "UNABLE_TO_GET_ISSUER_CERT"
        | "UNABLE_TO_GET_ISSUER_CERT_LOCALLY"
        | "UNABLE_TO_VERIFY_LEAF_SIGNATURE"
        | "UNABLE_TO_DECRYPT_CERT_SIGNATURE"
        | "CERT_CHAIN_TOO_LONG"
        | "INVALID_CA"
        | "HOSTNAME_MISMATCH"
        | "ERR_TLS_CERT_ALTNAME_INVALID" => Some(3),
        "ECONNRESET" | "ECONNABORTED" | "EPIPE" | "UND_ERR_SOCKET" => Some(4),
        "ENETUNREACH" | "EHOSTUNREACH" | "ENETDOWN" | "EHOSTDOWN" => Some(5),
        _ => None,
    }
}
const CLASSES: [(&str, &str); 7] = [
    ("WEB_FETCH_DNS_FAILED", "could not resolve the host name"),
    (
        "WEB_FETCH_CONNECTION_REFUSED",
        "the server refused the connection",
    ),
    ("WEB_FETCH_CONNECTION_TIMEOUT", "the connection timed out"),
    (
        "WEB_FETCH_CERTIFICATE_INVALID",
        "the server certificate could not be verified",
    ),
    (
        "WEB_FETCH_CONNECTION_INTERRUPTED",
        "the connection was interrupted",
    ),
    (
        "WEB_FETCH_NETWORK_UNREACHABLE",
        "the network or host is unreachable",
    ),
    ("WEB_FETCH_NETWORK_ERROR", "the network request failed"),
];
pub fn classify_transport_failure(error: &(dyn Error + 'static)) -> TransportFailure {
    let inner = innermost(error);
    let aggregate = inner
        .downcast_ref::<TransportError>()
        .and_then(|TransportError::Node { attempts, .. }| attempts.as_ref())
        .filter(|v| !v.is_empty());
    let attempts: Vec<_> = match aggregate {
        Some(attempts) => attempts.iter().map(|e| innermost(e.get_ref())).collect(),
        None => vec![inner],
    };
    let codes: Vec<_> = attempts.iter().map(|e| system_code(*e)).collect();
    let mut classes = Vec::new();
    for code in &codes {
        let kind = class(*code);
        if !classes.contains(&kind) {
            classes.push(kind);
        }
    }
    if classes.len() == 2 {
        classes.retain(|c| *c != Some(5));
    }
    let selected = if classes.len() == 1 {
        classes[0].unwrap_or(6)
    } else {
        6
    };
    let system = if codes.iter().all(|c| *c == codes[0]) {
        codes[0]
    } else {
        None
    };
    let detail = if aggregate.is_some() {
        let mut detail = attempts
            .iter()
            .take(4)
            .map(|e| message_of(*e, error))
            .collect::<Vec<_>>()
            .join("; ");
        if attempts.len() > 4 {
            detail.push_str(&format!("; {} more", attempts.len() - 4));
        }
        detail
    } else {
        message_of(inner, error)
    };
    let detail = if detail.encode_utf16().count() > 500 {
        let mut units = 0;
        let mut kept = String::new();
        for ch in detail.chars() {
            if units + ch.len_utf16() > 500 {
                break;
            }
            units += ch.len_utf16();
            kept.push(ch);
        }
        kept.push('…');
        kept
    } else {
        detail
    };
    let (code, summary) = CLASSES[selected];
    let suffix = system.map_or_else(String::new, |code| format!(" ({code})"));
    TransportFailure {
        code: code.into(),
        message: format!("{summary}{suffix}: {detail}"),
    }
}
