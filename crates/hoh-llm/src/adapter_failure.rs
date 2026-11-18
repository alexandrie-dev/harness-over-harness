use crate::{error::HarnessError, types::LlmFailure};
use std::error::Error;
/// Explicit structural bridge for SDK errors carrying an own code and failure snapshot.
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct AdapterFailure {
    pub message: String,
    pub code: String,
    pub failure: LlmFailure,
}
pub fn normalize_llm_failure(value: &(dyn Error + 'static)) -> LlmFailure {
    let carried = HarnessError::of(value)
        .and_then(|e| e.detail::<LlmFailure>().map(|f| (e.code(), f)))
        .or_else(|| {
            value
                .downcast_ref::<AdapterFailure>()
                .or_else(|| {
                    hoh_error::SharedError::of(value)
                        .and_then(|e| e.downcast_ref::<AdapterFailure>())
                })
                .map(|e| (e.code.as_str(), &e.failure))
        });
    if let Some((code, f)) = carried
        && code == f.code
        && !f.message.is_empty()
        && !f.code.is_empty()
        && f.status.is_none_or(|s| (100..=599).contains(&s))
        && f.provider_retry_after_ms
            .is_none_or(|n| n.is_finite() && n > 0.0)
        && f.request_id.as_ref().is_none_or(|s| !s.as_str().is_empty())
        && f.offload_images
            .is_none_or(|n| n > 0 && n <= 9_007_199_254_740_991)
    {
        return f.clone();
    }
    let message = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| value.to_string()))
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "LLM adapter failed".into());
    LlmFailure::new(
        message,
        HarnessError::of(value).map_or("UNKNOWN", HarnessError::code),
    )
}
pub fn normalize_llm_failure_value(value: &serde_json::Value) -> LlmFailure {
    let message = hoh_util_values::js_string(value);
    LlmFailure::new(
        if message.is_empty() {
            "LLM adapter failed".into()
        } else {
            message
        },
        "UNKNOWN",
    )
}
