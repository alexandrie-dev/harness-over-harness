//! Shared execution policy for LLM-backed session-title providers.

use std::{fmt, sync::Arc};

use futures_util::StreamExt;
use hoh_error::BoxError;
use hoh_kernel::Context;
use hoh_llm::{
    BlockAssembler, ContentBlock, FinishReason, GenerateOptions, Llm, LlmCallConfig,
    LlmCallControls, LlmResolvedModelInfo, MessageSource, NewUserMessage, ReasoningEffortId,
    RequestMessage, create_user_message,
};
use hoh_session::SessionSeq;
use hoh_session_title::{
    SessionTitleModelIdentity, SessionTitleProviderId, SessionTitleProviderRequest,
};
use hoh_timeout::{MAX_TIMER_DELAY_MS, deadline};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Capability-owned timeout reason code for auxiliary title requests.
pub const SESSION_TITLE_TIMEOUT_CODE: &str = "SESSION_TITLE_TIMEOUT";

/// Required execution controls for one model-backed title request.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionTitleLlmConfig {
    #[schemars(range(min = 1), extend("multipleOf" = 1))]
    pub max_input_bytes: f64,
    #[schemars(range(min = 1), extend("multipleOf" = 1))]
    pub max_output_tokens: f64,
    #[schemars(range(min = 1, max = 2_147_483_647), extend("multipleOf" = 1))]
    pub timeout_ms: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub model: Option<String>,
}

/// Validated immutable execution controls.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedSessionTitleLlmConfig {
    pub max_input_bytes: u64,
    pub max_output_tokens: u64,
    pub timeout_ms: f64,
    pub provider: Option<String>,
    pub model: Option<String>,
}

fn error(message: impl Into<String>) -> BoxError {
    message.into().into()
}

fn assert_positive_integer(name: &str, value: f64) -> Result<(), BoxError> {
    if !value.is_finite() || value <= 0.0 || value.fract() != 0.0 {
        return Err(error(format!(
            "session-title-llm: {name} must be a positive integer"
        )));
    }
    Ok(())
}

fn positive_integer(name: &str, value: f64) -> Result<u64, BoxError> {
    assert_positive_integer(name, value)?;
    if value >= u64::MAX as f64 {
        return Err(error(format!(
            "session-title-llm: {name} must be a positive integer"
        )));
    }
    Ok(value as u64)
}

/// Validate and detach required execution controls.
pub fn resolve_session_title_llm_config(
    config: &SessionTitleLlmConfig,
) -> Result<ResolvedSessionTitleLlmConfig, BoxError> {
    let max_input_bytes = positive_integer("maxInputBytes", config.max_input_bytes)?;
    let max_output_tokens = positive_integer("maxOutputTokens", config.max_output_tokens)?;
    assert_positive_integer("timeoutMs", config.timeout_ms)?;
    if config.timeout_ms > MAX_TIMER_DELAY_MS as f64 {
        return Err(error(format!(
            "session-title-llm: timeoutMs must not exceed {MAX_TIMER_DELAY_MS}"
        )));
    }
    if config.provider.is_some() != config.model.is_some() {
        return Err(error(
            "session-title-llm: provider and model must be supplied together",
        ));
    }
    if config
        .provider
        .as_ref()
        .zip(config.model.as_ref())
        .is_some_and(|(provider, model)| provider.is_empty() || model.is_empty())
    {
        return Err(error(
            "session-title-llm: provider and model overrides must be non-empty strings",
        ));
    }
    Ok(ResolvedSessionTitleLlmConfig {
        max_input_bytes,
        max_output_tokens,
        timeout_ms: config.timeout_ms,
        provider: config.provider.clone(),
        model: config.model.clone(),
    })
}

/// Validate configuration received as an untyped Loader value.
pub fn resolve_session_title_llm_config_value(
    candidate: &Value,
) -> Result<ResolvedSessionTitleLlmConfig, BoxError> {
    let empty_array = serde_json::Map::new();
    let value = match candidate {
        Value::Object(value) => value,
        Value::Array(values) if values.is_empty() => &empty_array,
        Value::Array(_) => {
            return Err(error("session-title-llm: unknown config key \"0\""));
        }
        _ => return Err(error("session-title-llm: configuration is required")),
    };
    for key in value.keys() {
        if !matches!(
            key.as_str(),
            "maxInputBytes" | "maxOutputTokens" | "timeoutMs" | "provider" | "model"
        ) {
            return Err(error(format!(
                "session-title-llm: unknown config key \"{key}\""
            )));
        }
    }
    fn number(value: &serde_json::Map<String, Value>, name: &str) -> f64 {
        value.get(name).and_then(Value::as_f64).unwrap_or(f64::NAN)
    }
    let max_input_bytes = number(value, "maxInputBytes");
    let max_output_tokens = number(value, "maxOutputTokens");
    let timeout_ms = number(value, "timeoutMs");
    positive_integer("maxInputBytes", max_input_bytes)?;
    positive_integer("maxOutputTokens", max_output_tokens)?;
    assert_positive_integer("timeoutMs", timeout_ms)?;
    if timeout_ms > MAX_TIMER_DELAY_MS as f64 {
        return Err(error(format!(
            "session-title-llm: timeoutMs must not exceed {MAX_TIMER_DELAY_MS}"
        )));
    }
    let has_provider = value.contains_key("provider");
    let has_model = value.contains_key("model");
    if has_provider != has_model {
        return Err(error(
            "session-title-llm: provider and model must be supplied together",
        ));
    }
    let (provider, model) = if has_provider {
        let pair = value
            .get("provider")
            .and_then(Value::as_str)
            .zip(value.get("model").and_then(Value::as_str));
        let Some((provider, model)) = pair else {
            return Err(error(
                "session-title-llm: provider and model overrides must be non-empty strings",
            ));
        };
        if provider.is_empty() || model.is_empty() {
            return Err(error(
                "session-title-llm: provider and model overrides must be non-empty strings",
            ));
        }
        (Some(provider.into()), Some(model.into()))
    } else {
        (None, None)
    };
    resolve_session_title_llm_config(&SessionTitleLlmConfig {
        max_input_bytes,
        max_output_tokens,
        timeout_ms,
        provider,
        model,
    })
}

/// Exact model-visible request recorded before one auxiliary title dispatch.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionTitleLlmRequestEventData {
    pub title_provider: SessionTitleProviderId,
    pub message_seqs: Vec<SessionSeq>,
    pub route: SessionTitleModelIdentity,
    pub system: String,
    pub messages: Vec<RequestMessage>,
    pub max_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<ReasoningEffortId>,
}

pub type SelectReasoningEffort =
    Arc<dyn Fn(&LlmResolvedModelInfo) -> Option<ReasoningEffortId> + Send + Sync>;

/// Provider-prepared model input, attribution, and reasoning selector.
#[derive(Clone)]
pub struct SessionTitleLlmPreparedRequest {
    pub system: String,
    pub input: String,
    pub message_seqs: Vec<SessionSeq>,
    pub select_reasoning_effort: SelectReasoningEffort,
}

/// Successful terminal finish kinds forwarded for provider interpretation.
#[derive(Clone, Debug, PartialEq)]
pub enum SessionTitleLlmFinish {
    Stop(FinishReason),
    ToolCalls(FinishReason),
    MaxTokens(FinishReason),
}

impl SessionTitleLlmFinish {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Stop(_) => "stop",
            Self::ToolCalls(_) => "tool-calls",
            Self::MaxTokens(_) => "max-tokens",
        }
    }

    pub fn as_finish_reason(&self) -> &FinishReason {
        match self {
            Self::Stop(finish) | Self::ToolCalls(finish) | Self::MaxTokens(finish) => finish,
        }
    }
}

/// Assembled auxiliary response for provider-owned interpretation.
#[derive(Clone, Debug, PartialEq)]
pub struct SessionTitleLlmResponse {
    pub blocks: Vec<ContentBlock>,
    pub finish: SessionTitleLlmFinish,
    pub model: SessionTitleModelIdentity,
}

/// Plain JavaScript-style operational error with the adapter failure code preserved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionTitleLlmFinishError {
    pub message: String,
    pub code: String,
}

impl fmt::Display for SessionTitleLlmFinishError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for SessionTitleLlmFinishError {}

fn resolve_route(
    config: &ResolvedSessionTitleLlmConfig,
    request: &SessionTitleProviderRequest,
) -> Result<SessionTitleModelIdentity, BoxError> {
    if let (Some(provider), Some(model)) = (&config.provider, &config.model) {
        return Ok(SessionTitleModelIdentity {
            provider: provider.clone(),
            model: model.clone(),
        });
    }
    request.route.clone().ok_or_else(|| {
        error(
            "session-title-llm: no logged request route is available; configure provider and model together",
        )
    })
}

fn terminal_finish(finish: FinishReason) -> Result<SessionTitleLlmFinish, BoxError> {
    match finish.kind.as_str() {
        "stop" => Ok(SessionTitleLlmFinish::Stop(finish)),
        "tool-calls" => Ok(SessionTitleLlmFinish::ToolCalls(finish)),
        "max-tokens" => Ok(SessionTitleLlmFinish::MaxTokens(finish)),
        "error" | "aborted" => {
            let failure = finish.failure().ok_or_else(|| {
                error(format!(
                    "session-title-llm: finish reason \"{}\" has no failure",
                    finish.kind
                ))
            })?;
            Err(Box::new(SessionTitleLlmFinishError {
                message: failure.message,
                code: failure.code,
            }))
        }
        _ => Err(error(format!(
            "unreachable variant in FinishReason: {}",
            serde_json::to_string(&finish).unwrap_or_else(|_| finish.kind.clone())
        ))),
    }
}

/// Execute one prepared auxiliary title request.
pub async fn execute_session_title_llm(
    ctx: &Context,
    config: &ResolvedSessionTitleLlmConfig,
    request: &SessionTitleProviderRequest,
    title_provider: SessionTitleProviderId,
    prepared: &SessionTitleLlmPreparedRequest,
) -> Result<SessionTitleLlmResponse, BoxError> {
    request.signal.throw_if_aborted()?;
    if prepared.message_seqs.is_empty() {
        return Err(error(
            "session-title-llm: at least one source message is required",
        ));
    }
    let input_bytes = u64::try_from(prepared.input.len()).unwrap_or(u64::MAX);
    if input_bytes > config.max_input_bytes {
        return Err(error(format!(
            "session-title-llm: input is {input_bytes} bytes, exceeding maxInputBytes {}",
            config.max_input_bytes
        )));
    }
    let route = resolve_route(config, request)?;
    let messages = vec![RequestMessage::from(create_user_message(NewUserMessage {
        content: vec![ContentBlock::text(&prepared.input)],
        source: MessageSource::new("dsh-session-title-llm"),
    }))];
    let call_deadline = deadline(
        Some(&request.signal),
        config.timeout_ms,
        SESSION_TITLE_TIMEOUT_CODE,
    )?;
    let runtime = ctx
        .get::<Llm>()
        .ok_or_else(|| error("session-title-llm: llm service is unavailable"))?;
    let selector = prepared.select_reasoning_effort.clone();
    let configure = move |controls: &LlmCallControls, model: &LlmResolvedModelInfo| {
        let mut controls = controls.clone();
        if let Some(effort) = selector(model) {
            controls.reasoning_effort = Some(effort);
        }
        Ok(controls)
    };
    let call = runtime
        .prepare_call(
            ctx,
            LlmCallConfig {
                provider: route.provider.clone(),
                model: route.model.clone(),
                controls: LlmCallControls {
                    max_tokens: Some(config.max_output_tokens),
                    ..Default::default()
                },
            },
            Some(call_deadline.signal.clone()),
            Some(&configure),
        )
        .await?;
    let options = GenerateOptions {
        messages: messages.clone(),
        system: Some(prepared.system.clone()),
        signal: Some(call_deadline.signal.clone()),
        session_id: Some(request.session.id().clone()),
        purpose: Some("session-title".into()),
        ..GenerateOptions::default().with_call_config(&call.config)
    };
    let event = SessionTitleLlmRequestEventData {
        title_provider,
        message_seqs: prepared.message_seqs.clone(),
        route: route.clone(),
        system: prepared.system.clone(),
        messages,
        max_tokens: config.max_output_tokens,
        reasoning_effort: call.config.controls.reasoning_effort.clone(),
    };
    request.session.append(
        "session/title-llm-request",
        serde_json::to_value(event)?,
        None,
    )?;
    call_deadline.signal.throw_if_aborted()?;
    let mut stream = call.stream(options)?;
    let mut assembler = BlockAssembler::new();
    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(chunk) => chunk,
            Err(cause) => {
                let _ = stream.close().await;
                return Err(cause);
            }
        };
        if let Err(reason) = call_deadline.signal.throw_if_aborted() {
            let _ = stream.close().await;
            return Err(Box::new(reason));
        }
        assembler.push(chunk);
    }
    call_deadline.signal.throw_if_aborted()?;
    let finish = terminal_finish(assembler.finish())?;
    let blocks = assembler.blocks()?;
    Ok(SessionTitleLlmResponse {
        blocks,
        finish,
        model: route,
    })
}
