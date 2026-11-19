//! `hoh/model/probe`: is there a usable model server behind `baseUrl`?
use crate::compat::{MAX_TOKENS, other_limit_field};
use crate::config::normalize_base_url;
use crate::error::{
    classify, error_fields, failure, http_error, redact, rejects_limit_field, retry_after_ms,
    truncate,
};
use crate::http::{authorized, client, guarded, model_ids, user_agent};
use hoh_timeout::AbortSignal;
use serde::Serialize;
use serde_json::{Value, json};
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct ProbeOptions {
    pub connect_timeout_ms: u64,
    pub models_timeout_ms: u64,
    pub completion_timeout_ms: u64,
}

impl Default for ProbeOptions {
    fn default() -> Self {
        Self {
            connect_timeout_ms: 5_000,
            models_timeout_ms: 5_000,
            // The first request after model start-up can wait for weights to load.
            completion_timeout_ms: 120_000,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ProbeError {
    pub code: String,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeResult {
    pub ok: bool,
    /// Duration of the one-token completion, 0 when it did not run.
    pub latency_ms: u64,
    /// The server accepted a request that declares a tool with `tool_choice`.
    /// It does not prove that the model calls tools well.
    pub tool_calls: bool,
    /// Ids from `GET /models`; empty when the list is unavailable.
    pub models: Vec<String>,
    /// `None` when the list was unavailable. Some servers list a file name, not the alias.
    pub model_listed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ProbeError>,
}

/// The diagnostic for the caller, with the API key in use removed from it.
fn error_of(error: &hoh_error::HarnessError, api_key: Option<&str>) -> ProbeError {
    let error = redact(error.clone(), api_key);
    ProbeError {
        code: error.code().to_owned(),
        message: error.message().to_owned(),
    }
}

/// Never fails: every problem is reported in the result.
pub async fn probe(
    base_url: &str,
    model: &str,
    api_key: Option<&str>,
    options: &ProbeOptions,
    signal: Option<&AbortSignal>,
) -> ProbeResult {
    let mut result = ProbeResult {
        ok: false,
        latency_ms: 0,
        tool_calls: false,
        models: Vec::new(),
        model_listed: None,
        error: None,
    };
    let base = match normalize_base_url(base_url) {
        Ok(base) => base,
        Err(error) => {
            result.error = Some(error_of(&error, api_key));
            return result;
        }
    };
    let http = match client(options.connect_timeout_ms) {
        Ok(http) => http,
        Err(error) => {
            result.error = Some(error_of(&error, api_key));
            return result;
        }
    };
    let agent = user_agent();
    let models_wait = Duration::from_millis(options.models_timeout_ms);
    let listing = async {
        let response = guarded(
            signal,
            models_wait,
            "model list",
            authorized(http.get(format!("{base}/models")), api_key, &agent).send(),
        )
        .await?;
        let status = response.status().as_u16();
        let text = guarded(signal, models_wait, "model list", response.text()).await?;
        if !(200..300).contains(&status) {
            return Err(http_error(status, &text, None, None));
        }
        Ok(serde_json::from_str::<Value>(&text)
            .map(|body| model_ids(&body, api_key))
            .unwrap_or_default())
    };
    match listing.await {
        Ok(ids) => {
            result.model_listed = (!ids.is_empty()).then(|| ids.iter().any(|id| id == model));
            result.models = ids;
        }
        // Unreachable server: the completion would only repeat the same failure.
        Err(error) if matches!(error.code(), "TRANSPORT" | "TIMEOUT" | "ABORTED") => {
            result.error = Some(error_of(&error, api_key));
            return result;
        }
        // An authentication failure or a missing /models route is reported by the completion.
        Err(_) => {}
    }

    let completion_wait = Duration::from_millis(options.completion_timeout_ms);
    let started = Instant::now();
    let mut probe = Completion {
        http: &http,
        base: &base,
        model,
        api_key,
        agent: &agent,
        wait: completion_wait,
        signal,
        limit_field: MAX_TOKENS,
    };
    let outcome = match probe.run(true).await {
        Ok(()) => {
            result.tool_calls = true;
            Ok(())
        }
        // Rejection of the tool declaration is not a reason to call the server unusable.
        Err(error) if matches!(error.code(), "INVALID_REQUEST") => probe.run(false).await,
        Err(error) => Err(error),
    };
    result.latency_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    match outcome {
        Ok(()) => result.ok = true,
        Err(error) => {
            result.error = Some(error_of(&error, api_key));
            result.latency_ms = 0;
        }
    }
    result
}

struct Completion<'a> {
    http: &'a reqwest::Client,
    base: &'a str,
    model: &'a str,
    api_key: Option<&'a str>,
    agent: &'a str,
    wait: Duration,
    signal: Option<&'a AbortSignal>,
    /// The output-limit field in use; switched once when the server refuses it.
    limit_field: &'static str,
}

impl Completion<'_> {
    /// One-token completion; a server that refuses the output-limit field is asked again with
    /// the other spelling (`max_tokens` / `max_completion_tokens`).
    async fn run(&mut self, with_tool: bool) -> Result<(), hoh_error::HarnessError> {
        match self.send(with_tool).await {
            Err(error) if rejects_limit_field(&error, self.limit_field) => {
                self.limit_field = other_limit_field(self.limit_field);
                self.send(with_tool).await
            }
            other => other,
        }
    }

    async fn send(&self, with_tool: bool) -> Result<(), hoh_error::HarnessError> {
        let mut body = json!({
            "model": self.model,
            "messages": [{"role": "user", "content": "ping"}],
            "stream": false,
        });
        body[self.limit_field] = json!(1);
        if with_tool {
            body["tools"] = json!([{"type": "function", "function": {
                "name": "probe", "description": "Probe tool.",
                "parameters": {"type": "object", "properties": {}}}}]);
            body["tool_choice"] = json!("auto");
        }
        let request = authorized(
            self.http.post(format!("{}/chat/completions", self.base)),
            self.api_key,
            self.agent,
        )
        .header("content-type", "application/json")
        .body(body.to_string());
        let response = guarded(self.signal, self.wait, "completion", request.send()).await?;
        let status = response.status().as_u16();
        let retry = response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(retry_after_ms);
        let text = guarded(self.signal, self.wait, "completion", response.text()).await?;
        if !(200..300).contains(&status) {
            return Err(http_error(status, &text, retry, None));
        }
        check_completion(&text)
    }
}

/// A 200 is not enough: an HTML page of a proxy, `{}` or an error object in the body mean that
/// no model answered. A Chat Completions reply has a non-empty `choices` list.
fn check_completion(text: &str) -> Result<(), hoh_error::HarnessError> {
    let Ok(body) = serde_json::from_str::<Value>(text) else {
        return Err(failure(
            hoh_llm::BuiltinLlmFailureCode::MalformedResponse,
            format!(
                "the server answered 200 but not with a Chat Completions JSON body: {}",
                truncate(text.trim(), 200)
            ),
        ));
    };
    if body.get("choices").is_none()
        && let Some(fields) = error_fields(&body)
    {
        return Err(classify(&fields, None, None, None));
    }
    let answered = body
        .get("choices")
        .and_then(Value::as_array)
        .is_some_and(|choices| choices.iter().any(Value::is_object));
    if answered {
        Ok(())
    } else {
        Err(failure(
            hoh_llm::BuiltinLlmFailureCode::MalformedResponse,
            format!(
                "the server answered 200 but the body has no Chat Completions `choices`: {}",
                truncate(text.trim(), 200)
            ),
        ))
    }
}
