//! packages/sdk/server/src/user-questions.ts: native answerer and cancellation bridge.
use crate::{transport::JsonRpcTransportPeer, user_question_helpers::UserQuestionError};
use hoh_agent::Agent;
use hoh_error::{BoxError, HarnessError};
use hoh_kernel::{Context, Disposer};
use hoh_protocol::{SdkUserQuestionAnswer, SdkUserQuestionRequest};
use hoh_timeout::{AbortController, AbortSignal};
use serde_json::Value;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

use hoh_user_questions::AskUserQuestionAnswer;
/// The owner crate's request event (`user-questions/request`), answered through a waterfall.
pub use hoh_user_questions::AskUserQuestionRequest;

/// Same JSON on both sides (`deny_unknown_fields`, camelCase); the wire type lives in hoh-protocol.
fn convert<T: serde::Serialize, U: serde::de::DeserializeOwned>(value: &T) -> Result<U, BoxError> {
    Ok(serde_json::from_value(serde_json::to_value(value)?)?)
}

pub struct SdkUserQuestionBridge {
    enabled: Arc<AtomicBool>,
    shutdown: Arc<AbortController>,
    unregister: Mutex<Option<Disposer>>,
}
impl SdkUserQuestionBridge {
    pub fn new(
        ctx: &Context,
        transport: Arc<dyn JsonRpcTransportPeer>,
        owns_agent: impl Fn(&Arc<dyn Agent>) -> bool + Send + Sync + 'static,
    ) -> Result<Self, BoxError> {
        let enabled = Arc::new(AtomicBool::new(false));
        let shutdown = Arc::new(AbortController::new());
        let (enabled_listener, shutdown_listener, owner) =
            (enabled.clone(), shutdown.clone(), ctx.clone());
        let unregister = ctx.on_waterfall::<AskUserQuestionRequest, AskUserQuestionAnswer, _>(move |request, next| {
            let (enabled, shutdown, ctx, transport) = (enabled_listener.clone(), shutdown_listener.clone(), owner.clone(), transport.clone());
            let owned = enabled.load(Ordering::SeqCst) && request.agent.as_ref().is_some_and(&owns_agent);
            async move {
                if !enabled.load(Ordering::SeqCst) || !owned { return next.run().await; }
                // The ownership predicate above established presence.
                let agent = request.agent.as_ref().ok_or("missing question Agent")?;
                let params = SdkUserQuestionRequest { request_id: hoh_util_crypto::random_uuid().to_string(), session_id: agent.session().id().to_string(), questions: convert(&request.questions)? };
                let signal = request.signal.as_ref().map_or_else(|| shutdown.signal(), |s| AbortSignal::any([s.clone(), shutdown.signal()]));
                signal.throw_if_aborted()?;
                let result = transport.request("user-question/ask", serde_json::to_value(&params)?, Some(signal.clone()));
                // The request starts before cancellation admission; no task survives this call.
                let result = tokio::select! {
                    biased;
                    reason = signal.cancelled() => {
                        if let Err(e) = transport.notify("user-question.cancelled", Some(serde_json::to_value(hoh_protocol::SdkUserQuestionCancelled { request_id: params.request_id, session_id: params.session_id })?)) {
                            ctx.logger().warn(format!("SDK question cancellation could not reach the host: {}", crate::error_string(e.as_ref())));
                        }
                        Err(Box::new(reason) as BoxError)
                    }
                    result = result => result,
                };
                match result {
                    Ok(value) => parse_host_answer(value, &request.questions.iter().map(|q| q.id.as_str()).collect::<Vec<_>>()).and_then(|answer| convert(&answer)),
                    Err(error) => {
                        if HarnessError::of(error.as_ref()).is_some_and(|e| e.is_named(UserQuestionError::NAME)) { return Err(error); }
                        let cancelled = signal.aborted();
                        Err(UserQuestionError::new(if cancelled { "SDK user question was cancelled" } else { "SDK host could not answer the user question" }, if cancelled { "ASK_ABORTED" } else { "PROVIDER_ERROR" }).with_cause(error).into())
                    }
                }
            }
        })?;
        Ok(Self {
            enabled,
            shutdown,
            unregister: Mutex::new(Some(unregister)),
        })
    }
    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::SeqCst);
    }
    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::SeqCst)
    }
    pub fn close(&self) {
        self.set_enabled(false);
        if let Some(unregister) = crate::lock(&self.unregister).take() {
            drop(unregister.dispose());
        }
        self.shutdown.abort_with(UserQuestionError::new(
            "SDK server is shutting down",
            "ASK_ABORTED",
        ));
    }
}

pub fn parse_host_answer(
    value: Value,
    question_ids: &[&str],
) -> Result<SdkUserQuestionAnswer, BoxError> {
    let bad = || -> BoxError {
        UserQuestionError::new("SDK host must answer each question id exactly once with selected labels and optional custom text", "BAD_ANSWER").into()
    };
    if let Some(object) = value.as_object()
        && object.contains_key("cancelled")
    {
        if object["cancelled"] != true || object.len() != 1 {
            return Err(bad());
        }
        return Err(
            UserQuestionError::new("The user dismissed the SDK question", "ASK_CANCELLED").into(),
        );
    }
    let values = value
        .get("answers")
        .and_then(Value::as_array)
        .ok_or_else(bad)?;
    let mut answers = Vec::with_capacity(values.len());
    let mut ids = std::collections::HashSet::new();
    for value in values {
        let object = value.as_object().ok_or_else(bad)?;
        let id = object.get("id").and_then(Value::as_str).ok_or_else(bad)?;
        let selected = object
            .get("selected")
            .and_then(Value::as_array)
            .ok_or_else(bad)?;
        if !ids.insert(id)
            || selected.iter().any(|v| !v.is_string())
            || object.get("custom").is_some_and(|v| !v.is_string())
        {
            return Err(bad());
        }
        answers.push(hoh_protocol::AskUserQuestionAnswerItem {
            id: id.into(),
            selected: selected
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect(),
            custom: object
                .get("custom")
                .and_then(Value::as_str)
                .map(str::to_owned),
        });
    }
    if answers.len() != question_ids.len() || question_ids.iter().any(|id| !ids.contains(id)) {
        return Err(bad());
    }
    Ok(SdkUserQuestionAnswer { answers })
}
