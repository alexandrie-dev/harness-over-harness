//! Browser-facing control validation and stable remote failures.

use hoh_attachment::AttachmentError;
use hoh_error::{BoxError, HarnessError};
use hoh_session::SessionId;
use hoh_timeout::AbortSignal;
use serde_json::{Value, json};

use crate::error::SubagentError;

/// Validate fields stricter than branded-string decoding.
pub fn validate_control_request(method: &str, payload: &Value) -> Result<(), HarnessError> {
    let object = payload.as_object();
    let child_id = object
        .and_then(|object| object.get("childSessionId"))
        .and_then(Value::as_str);
    let delivery = object
        .and_then(|object| object.get("delivery"))
        .and_then(Value::as_str);
    let valid = match method {
        "subagent.prompt" => {
            child_id.is_some_and(|value| !value.is_empty())
                && matches!(delivery, Some("queue" | "steer"))
        }
        "subagent.interrupt" => child_id.is_some_and(|value| !value.is_empty()),
        _ => false,
    };
    if valid {
        return Ok(());
    }
    let issues = manual_zod_issues(method, payload);
    Err(remote_error(
        "gateway/bad-request",
        format!("invalid payload for {method}"),
        json!({ "issues": issues }),
        None,
    ))
}

/// Map one prompt admission failure to the public RemoteError vocabulary.
pub fn reject_prompt(
    error: BoxError,
    child_session_id: &SessionId,
    signal: &AbortSignal,
) -> HarnessError {
    let harness = HarnessError::of(error.as_ref());
    let harness_name = harness.map(HarnessError::name);
    let harness_code = harness.map(HarnessError::code).map(str::to_owned);
    let harness_message = harness.map(HarnessError::message).map(str::to_owned);
    let subagent_code = harness
        .filter(|value| value.is_named(SubagentError::NAME))
        .map(HarnessError::code)
        .map(str::to_owned);
    if signal.aborted() || subagent_code.as_deref() == Some("CANCELLED") {
        return remote_error(
            "gateway/cancelled",
            "subagent prompt was cancelled",
            json!({}),
            Some(error),
        );
    }
    if harness_name == Some(AttachmentError::NAME) {
        let reason = harness_code.as_deref().unwrap_or_default();
        let message = harness_message.as_deref().unwrap_or_default();
        return remote_error(
            "subagent/attachment-invalid",
            message,
            json!({ "reason": reason }),
            Some(error),
        );
    }
    let id = child_session_id.as_str();
    match subagent_code.as_deref() {
        Some("MODEL_DOES_NOT_SUPPORT_IMAGES") => remote_error(
            "subagent/attachment-invalid",
            harness_message.as_deref().unwrap_or_default(),
            json!({ "reason": "MODEL_DOES_NOT_SUPPORT_IMAGES" }),
            Some(error),
        ),
        Some("NOT_RESUMABLE") => remote_error(
            "subagent/not-resumable",
            "subagent cannot be resumed",
            json!({ "childSessionId": id }),
            Some(error),
        ),
        Some("UNAUTHORIZED") => remote_error(
            "subagent/unauthorized",
            "subagent membership in the active tree could not be verified",
            json!({ "childSessionId": id }),
            Some(error),
        ),
        Some(
            "DRAINING"
            | "ACTIVATION_CLOSING"
            | "ACTIVATION_LIMIT_REACHED"
            | "CONTINUATION_UNAVAILABLE"
            | "PERSISTENCE_UNAVAILABLE",
        ) => remote_error(
            "subagent/delivery-unavailable",
            "subagent follow-up is temporarily unavailable",
            json!({ "childSessionId": id }),
            Some(error),
        ),
        _ => remote_error(
            "gateway/internal",
            "subagent prompt failed",
            json!({}),
            Some(error),
        ),
    }
}

fn remote_error(
    code: &'static str,
    message: impl Into<String>,
    details: Value,
    cause: Option<BoxError>,
) -> HarnessError {
    let error = HarnessError::named("RemoteError", message, code).with_detail(details);
    cause.map_or(error.clone(), |cause| error.with_cause(cause))
}

// Hand-written zod 4 issue shape: this text/data is user-visible through RemoteError details.
fn manual_zod_issues(method: &str, payload: &Value) -> Vec<Value> {
    let mut issues = Vec::new();
    let object = payload.as_object();
    let child = object.and_then(|value| value.get("childSessionId"));
    if !child
        .and_then(Value::as_str)
        .is_some_and(|value| !value.is_empty())
    {
        issues.push(json!({
            "origin": "string",
            "code": "too_small",
            "minimum": 1,
            "inclusive": true,
            "path": ["childSessionId"],
            "message": "Too small: expected string to have >=1 characters"
        }));
    }
    if method == "subagent.prompt"
        && !matches!(
            object
                .and_then(|value| value.get("delivery"))
                .and_then(Value::as_str),
            Some("queue" | "steer")
        )
    {
        issues.push(json!({
            "code": "invalid_value",
            "values": ["queue", "steer"],
            "path": ["delivery"],
            "message": "Invalid option: expected one of \"queue\"|\"steer\""
        }));
    }
    issues
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty_child_id_before_dispatch() {
        let error = validate_control_request(
            "subagent.prompt",
            &json!({ "childSessionId": "", "delivery": "queue" }),
        )
        .unwrap_err();
        assert_eq!(error.name(), "RemoteError");
        assert_eq!(error.code(), "gateway/bad-request");
        assert_eq!(error.message(), "invalid payload for subagent.prompt");
    }

    #[test]
    fn accepts_exact_control_vocabulary() {
        validate_control_request(
            "subagent.prompt",
            &json!({ "childSessionId": "child", "delivery": "steer" }),
        )
        .unwrap();
        validate_control_request("subagent.interrupt", &json!({ "childSessionId": "child" }))
            .unwrap();
    }
}
