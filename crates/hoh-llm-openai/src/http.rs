//! Shared HTTP pieces: client, headers, model list parsing, abort/timeout guard.
use crate::error::failure;
use hoh_error::{HarnessError, SharedError};
use hoh_llm::{BuiltinLlmFailureCode as Code, LlmErrorOptions};
use hoh_timeout::AbortSignal;
use serde_json::Value;
use std::future::Future;
use std::time::Duration;

pub fn client(connect_timeout_ms: u64) -> Result<reqwest::Client, HarnessError> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_millis(connect_timeout_ms))
        // A local model server is addressed directly; environment proxies would only get in the way.
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| {
            failure(
                Code::InvalidConfig,
                format!("HTTP client cannot be built: {e}"),
            )
        })
}

pub fn user_agent() -> String {
    hoh_llm::attribution_headers(None)
        .ok()
        .and_then(|mut headers| headers.shift_remove("user-agent"))
        .unwrap_or_else(|| "hoh".into())
}

pub fn authorized(
    request: reqwest::RequestBuilder,
    api_key: Option<&str>,
    user_agent: &str,
) -> reqwest::RequestBuilder {
    let request = request.header("user-agent", user_agent);
    match api_key {
        Some(key) => request.header("authorization", format!("Bearer {key}")),
        None => request,
    }
}

/// Ids from a `GET /models` body: `{"data": [{"id": ..}]}`; Ollama's native `models` list also.
/// A model whose id contains the API key in use is dropped: the id would carry the key into the
/// model list and the probe result.
pub fn model_ids(body: &Value, api_key: Option<&str>) -> Vec<String> {
    ["data", "models"]
        .iter()
        .find_map(|key| body.get(*key).and_then(Value::as_array))
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            entry
                .get("id")
                .or_else(|| entry.get("name"))
                .and_then(Value::as_str)
        })
        .filter(|id| !crate::error::mentions_secret(id, api_key))
        .map(str::to_owned)
        .collect()
}

pub fn aborted(reason: Option<SharedError>) -> HarnessError {
    failure_with_cause(Code::Aborted, "OpenAI-compatible request aborted", reason)
}

pub fn failure_with_cause(code: Code, message: &str, cause: Option<SharedError>) -> HarnessError {
    crate::error::failure_with(
        code,
        message.to_owned(),
        LlmErrorOptions {
            cause,
            ..Default::default()
        },
    )
}

pub fn transport(error: &reqwest::Error) -> HarnessError {
    failure(
        Code::Transport,
        format!("OpenAI-compatible request failed: {error}"),
    )
}

/// Run one network wait with the caller's abort signal and an idle limit.
pub async fn guarded<T, F>(
    signal: Option<&AbortSignal>,
    idle: Duration,
    what: &str,
    future: F,
) -> Result<T, HarnessError>
where
    F: Future<Output = Result<T, reqwest::Error>>,
{
    let abort = async {
        match signal {
            Some(signal) => Some(signal.cancelled().await),
            None => std::future::pending().await,
        }
    };
    tokio::select! {
        biased;
        reason = abort => Err(aborted(reason)),
        result = tokio::time::timeout(idle, future) => match result {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(error)) => Err(transport(&error)),
            Err(_) => Err(failure(
                Code::Timeout,
                format!("OpenAI-compatible {what} idle timeout ({} ms without data)", idle.as_millis()),
            )),
        },
    }
}
