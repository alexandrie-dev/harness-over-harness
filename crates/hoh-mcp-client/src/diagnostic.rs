use hoh_error::{BoxError, HarnessError};
use rmcp::ServiceError;
use std::{collections::HashSet, error::Error};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum McpLocalErrorCode {
    ConnectionClosed,
    Timeout,
    CredentialsUnavailable,
    CredentialNotFound,
    InstructionsTooLarge,
    ToolRegistrationFailed,
    UnclosedGeneration,
}
impl McpLocalErrorCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ConnectionClosed => "MCP_CONNECTION_CLOSED",
            Self::Timeout => "MCP_TIMEOUT",
            Self::CredentialsUnavailable => "MCP_CREDENTIALS_UNAVAILABLE",
            Self::CredentialNotFound => "MCP_CREDENTIAL_NOT_FOUND",
            Self::InstructionsTooLarge => "MCP_INSTRUCTIONS_TOO_LARGE",
            Self::ToolRegistrationFailed => "MCP_TOOL_REGISTRATION_FAILED",
            Self::UnclosedGeneration => "MCP_UNCLOSED_GENERATION",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum McpToolErrorCode {
    ToolError,
    UnsupportedTaskExecution,
    InvalidResult,
    ConnectionClosed,
    Timeout,
    ServerUnavailable,
    RequestFailed,
}
impl McpToolErrorCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ToolError => "MCP_TOOL_ERROR",
            Self::UnsupportedTaskExecution => "MCP_UNSUPPORTED_TASK_EXECUTION",
            Self::InvalidResult => "MCP_INVALID_RESULT",
            Self::ConnectionClosed => "MCP_CONNECTION_CLOSED",
            Self::Timeout => "MCP_TIMEOUT",
            Self::ServerUnavailable => "MCP_SERVER_UNAVAILABLE",
            Self::RequestFailed => "MCP_REQUEST_FAILED",
        }
    }
}

pub struct McpLocalError;
#[allow(clippy::new_ret_no_self)]
impl McpLocalError {
    pub fn new(message: impl Into<String>, code: McpLocalErrorCode) -> HarnessError {
        HarnessError::named("McpLocalError", message, code.as_str())
    }
    pub fn with_cause(
        message: impl Into<String>,
        code: McpLocalErrorCode,
        cause: impl Into<BoxError>,
    ) -> HarnessError {
        Self::new(message, code).with_cause(cause)
    }
    pub fn is(error: &(dyn Error + 'static)) -> bool {
        HarnessError::of(error).is_some_and(|error| error.is_named("McpLocalError"))
    }
}

pub struct McpToolError;
#[allow(clippy::new_ret_no_self)]
impl McpToolError {
    pub fn new(message: impl Into<String>, code: McpToolErrorCode) -> HarnessError {
        HarnessError::named("McpToolError", message, code.as_str())
    }
    pub fn with_cause(
        message: impl Into<String>,
        code: McpToolErrorCode,
        cause: impl Into<BoxError>,
    ) -> HarnessError {
        Self::new(message, code).with_cause(cause)
    }
}

pub fn disconnected_message(server: &str) -> String {
    format!("mcp-client({server}): server is disconnected")
}

pub fn mcp_request_error(
    error: BoxError,
    operation: &str,
    redact: Option<&(dyn Fn(&str) -> String + Send + Sync)>,
) -> HarnessError {
    if McpLocalError::is(error.as_ref())
        && let Some(existing) = HarnessError::of(error.as_ref())
    {
        return existing.clone();
    }
    let mut code = if error
        .downcast_ref::<std::io::Error>()
        .is_some_and(|error| error.kind() == std::io::ErrorKind::NotConnected)
    {
        McpToolErrorCode::ServerUnavailable
    } else {
        McpToolErrorCode::RequestFailed
    };
    let detail = if let Some(service) = error.downcast_ref::<ServiceError>() {
        match service {
            ServiceError::McpError(protocol) => {
                format!("JSON-RPC error {}: {}", protocol.code.0, protocol.message)
            }
            ServiceError::TransportClosed => {
                code = McpToolErrorCode::ConnectionClosed;
                service.to_string()
            }
            ServiceError::Timeout { .. } => {
                code = McpToolErrorCode::Timeout;
                service.to_string()
            }
            _ => service.to_string(),
        }
    } else {
        describe_error(error.as_ref())
    };
    let message = format!("MCP {operation} failed: {detail}");
    if let Some(redact) = redact {
        McpToolError::new(redact(&message), code)
    } else {
        McpToolError::with_cause(message, code, error)
    }
}

pub fn redact_secrets(text: &str, secrets: impl IntoIterator<Item = impl AsRef<str>>) -> String {
    let mut secrets: Vec<String> = secrets
        .into_iter()
        .map(|s| s.as_ref().to_owned())
        .filter(|s| !s.is_empty())
        .collect();
    secrets.sort_by_key(|secret| std::cmp::Reverse(secret.len()));
    secrets.dedup();
    secrets.into_iter().fold(text.to_owned(), |text, secret| {
        text.replace(&secret, "[redacted]")
    })
}

pub fn describe_error(error: &(dyn Error + 'static)) -> String {
    let mut seen = HashSet::new();
    let mut parts = vec![error.to_string()];
    let mut current = error.source();
    while let Some(cause) = current {
        let address = std::ptr::from_ref(cause).cast::<()>() as usize;
        if !seen.insert(address) {
            break;
        }
        parts.push(cause.to_string());
        current = cause.source();
    }
    let mut result = parts.remove(0);
    for part in parts {
        result.push_str(", cause: ");
        result.push_str(&part);
    }
    result
}
