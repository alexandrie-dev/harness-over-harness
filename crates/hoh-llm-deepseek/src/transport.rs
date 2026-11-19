//! Normalize HTTP and in-band Messages errors.

use hoh_error::HarnessError;
use hoh_llm::{
    BuiltinLlmFailureCode as Code, LlmError, LlmErrorOptions, ProviderRequestId,
    is_context_window_exceeded_error, is_quota_exceeded_error,
};
use serde_json::Value;

pub fn provider_error_detail(raw: &Value) -> String {
    let Some(error) = raw.get("error").and_then(Value::as_object) else {
        return String::new();
    };
    ["code", "type", "message"]
        .iter()
        .filter_map(|key| error.get(*key).and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join(" ")
}

fn retry_after_ms_at(value: &str, now_ms: i64) -> Option<f64> {
    let decimal_seconds = || {
        let (integer, fraction) = value
            .split_once('.')
            .map_or((value, None), |(integer, fraction)| {
                (integer, Some(fraction))
            });
        (!integer.is_empty()
            && integer.bytes().all(|byte| byte.is_ascii_digit())
            && fraction.is_none_or(|fraction| {
                !fraction.is_empty() && fraction.bytes().all(|byte| byte.is_ascii_digit())
            }))
        .then(|| value.parse::<f64>().ok())
        .flatten()
    };
    if let Some(seconds) = decimal_seconds() {
        return (seconds.is_finite() && seconds > 0.0).then_some(seconds * 1000.0);
    }
    let target_ms = jiff::fmt::rfc2822::parse(value)
        .ok()?
        .timestamp()
        .as_millisecond();
    let delay = target_ms.checked_sub(now_ms)? as f64;
    (delay.is_finite() && delay > 0.0).then_some(delay)
}

fn retry_after_ms(value: &str) -> Option<f64> {
    retry_after_ms_at(value, jiff::Timestamp::now().as_millisecond())
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::*;

    #[test]
    fn retry_after_accepts_delta_seconds_and_future_http_dates() {
        assert_eq!(retry_after_ms_at("1.5", 0), Some(1_500.0));
        let now = "2037-10-21T07:27:00Z"
            .parse::<jiff::Timestamp>()
            .unwrap()
            .as_millisecond();
        assert_eq!(
            retry_after_ms_at("Wed, 21 Oct 2037 07:28:00 GMT", now),
            Some(60_000.0)
        );
        assert_eq!(
            retry_after_ms_at("Wed, 21 Oct 2015 07:28:00 GMT", now),
            None
        );
        for rejected in ["1e2", "+1", " 1", "1 ", "1.", ".5"] {
            assert_eq!(retry_after_ms_at(rejected, 0), None, "{rejected:?}");
        }
    }
}

pub fn provider_error(
    raw: &Value,
    status: Option<u16>,
    headers: Option<&reqwest::header::HeaderMap>,
) -> HarnessError {
    let error = raw.get("error").and_then(Value::as_object);
    let message = error
        .and_then(|value| value.get("message"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| {
            format!(
                "DeepSeek Messages request failed ({})",
                status.map_or_else(|| "stream error".into(), |value| value.to_string())
            )
        });
    let kind = error
        .and_then(|value| value.get("type"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let detail = format!(
        "{kind} {} {message}",
        error
            .and_then(|value| value.get("code"))
            .and_then(Value::as_str)
            .unwrap_or("")
    );
    let code = if matches!(status, Some(401 | 403))
        || matches!(kind, "authentication_error" | "permission_error")
    {
        Code::Auth
    } else if is_quota_exceeded_error(&detail) || status == Some(402) {
        Code::Quota
    } else if status == Some(429) || kind == "rate_limit_error" {
        Code::RateLimit
    } else if is_context_window_exceeded_error(&detail) {
        Code::ContextWindowExceeded
    } else if matches!(status, Some(400 | 413)) || kind == "invalid_request_error" {
        Code::InvalidRequest
    } else if status.is_some_and(|value| value >= 500)
        || matches!(kind, "api_error" | "overloaded_error")
    {
        Code::Server
    } else {
        status.map_or(Code::Server, Code::Http)
    };
    let header = |name: &str| {
        headers?
            .get(name)?
            .to_str()
            .ok()
            .filter(|value| !value.is_empty())
    };
    let request_id = header("request-id")
        .or_else(|| header("x-request-id"))
        .or_else(|| header("x-deepseek-request-id"))
        .map(ProviderRequestId::from);
    let provider_retry_after_ms = header("retry-after").and_then(retry_after_ms);
    LlmError::with_options(
        message,
        code,
        LlmErrorOptions {
            status,
            request_id,
            provider_retry_after_ms,
            ..Default::default()
        },
    )
    .expect("provider failure is valid")
}
