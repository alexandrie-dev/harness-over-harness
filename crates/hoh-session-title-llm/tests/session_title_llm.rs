use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

use futures_util::stream;
use hoh_error::{BoxError, HarnessError};
use hoh_kernel::{Context, Kernel};
use hoh_llm::{
    FinishReason, GenerateOptions, Llm, LlmAdapter, LlmFailure, LlmFuture, LlmModelInfo,
    LlmModelReasoningInfo, LlmPlugin, LlmReasoningEffortInfo, LlmResolvedModelInfo, LlmStream,
    LlmStreamEvent, ReasoningEffortId, StreamChunk,
};
use hoh_session::{Session, SessionSeq};
use hoh_session_title::{
    SessionTitleModelIdentity, SessionTitleProviderRequest, SessionTitleUserMessage,
};
use hoh_session_title_llm::{
    SESSION_TITLE_TIMEOUT_CODE, SessionTitleLlmConfig, SessionTitleLlmFinish,
    SessionTitleLlmFinishError, SessionTitleLlmPreparedRequest, execute_session_title_llm,
    resolve_session_title_llm_config, resolve_session_title_llm_config_value,
};
use hoh_timeout::{AbortController, AbortError, MAX_TIMER_DELAY_MS, TimeoutReason, timeout_of};
use schemars::schema_for;
use serde_json::{Value, json};
use tokio::sync::Notify;

fn script(finish: FinishReason) -> Vec<StreamChunk> {
    vec![
        serde_json::from_value(json!({"type":"block-start","index":0,"blockType":"text"})).unwrap(),
        StreamChunk::TextDelta {
            index: 0,
            text: "  五个字标题  ".into(),
            wire: Default::default(),
        },
        StreamChunk::finish(finish),
    ]
}

struct RecordingAdapter {
    script: Vec<StreamChunk>,
    requests: Mutex<Vec<GenerateOptions>>,
    reasoning_efforts: Vec<ReasoningEffortId>,
    session: Mutex<Option<Session>>,
    logged_at_dispatch: AtomicBool,
}

impl RecordingAdapter {
    fn new(finish: FinishReason, efforts: &[&str]) -> Arc<Self> {
        Self::with_script(script(finish), efforts)
    }

    fn with_script(script: Vec<StreamChunk>, efforts: &[&str]) -> Arc<Self> {
        Arc::new(Self {
            script,
            requests: Mutex::new(vec![]),
            reasoning_efforts: efforts.iter().map(|value| (*value).into()).collect(),
            session: Mutex::new(None),
            logged_at_dispatch: AtomicBool::new(false),
        })
    }
}

impl LlmAdapter for RecordingAdapter {
    fn resolve_model<'a>(
        &'a self,
        provider: &'a str,
        model: &'a str,
        _signal: Option<&'a hoh_timeout::AbortSignal>,
    ) -> LlmFuture<'a, LlmResolvedModelInfo> {
        let efforts = self.reasoning_efforts.clone();
        Box::pin(async move {
            Ok(LlmResolvedModelInfo {
                model: LlmModelInfo {
                    provider: provider.into(),
                    id: model.into(),
                    name: model.into(),
                    ..Default::default()
                },
                reasoning: (!efforts.is_empty()).then(|| LlmModelReasoningInfo {
                    efforts: efforts
                        .into_iter()
                        .map(|id| LlmReasoningEffortInfo {
                            name: id.to_string(),
                            id,
                            description: None,
                        })
                        .collect(),
                    default_effort: None,
                }),
                ..Default::default()
            })
        })
    }

    fn stream(&self, options: GenerateOptions) -> Result<LlmStream, BoxError> {
        let logged = self
            .session
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|session| {
                session
                    .snapshot_events(None, None)
                    .iter()
                    .any(|event| event.event_type() == "session/title-llm-request")
            });
        self.logged_at_dispatch.store(logged, Ordering::SeqCst);
        self.requests.lock().unwrap().push(options);
        Ok(LlmStream::chunks(self.script.clone()))
    }
}

struct DelayedAdapter {
    delay_ms: u64,
}

struct CooperativeAdapter;

impl LlmAdapter for CooperativeAdapter {
    fn stream(&self, options: GenerateOptions) -> Result<LlmStream, BoxError> {
        let signal = options.signal.expect("title request signal");
        Ok(LlmStream::new(stream::once(async move {
            let reason = signal.cancelled().await;
            Err(Box::new(reason) as BoxError)
        })))
    }
}

impl LlmAdapter for DelayedAdapter {
    fn stream(&self, _options: GenerateOptions) -> Result<LlmStream, BoxError> {
        let delay_ms = self.delay_ms;
        Ok(LlmStream::new(stream::once(async move {
            tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
            Ok(StreamChunk::finish(FinishReason::new("stop")))
        })))
    }
}

async fn context(adapter: Arc<dyn LlmAdapter>, providers: &[&str]) -> (Kernel, Context) {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    ctx.plugin::<LlmPlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    ctx.get::<Llm>()
        .unwrap()
        .register_adapter(
            &ctx,
            providers
                .iter()
                .map(|provider| (*provider).into())
                .collect(),
            adapter,
        )
        .unwrap();
    (kernel, ctx)
}

fn make_request_with_conversation_cap(
    signal: hoh_timeout::AbortSignal,
    max_tokens: Option<u64>,
) -> SessionTitleProviderRequest {
    let session = Session::create("title-test".into(), None, None, None, None).unwrap();
    if let Some(max_tokens) = max_tokens {
        session
            .append(
                "request/header",
                json!({
                    "header": {
                        "config": {
                            "provider": "current-route",
                            "model": "current-model",
                            "maxTokens": max_tokens,
                        }
                    },
                    "reason": "initial",
                }),
                None,
            )
            .unwrap();
    }
    SessionTitleProviderRequest {
        session,
        messages: vec![SessionTitleUserMessage {
            seq: SessionSeq::new(1).unwrap(),
            text: "first prompt".into(),
        }],
        route: Some(SessionTitleModelIdentity {
            provider: "current-route".into(),
            model: "current-model".into(),
        }),
        current_title: None,
        signal,
    }
}

fn make_request(signal: hoh_timeout::AbortSignal) -> SessionTitleProviderRequest {
    make_request_with_conversation_cap(signal, None)
}

fn config() -> SessionTitleLlmConfig {
    SessionTitleLlmConfig {
        max_input_bytes: 1_000.0,
        max_output_tokens: 32.0,
        timeout_ms: 1_000.0,
        provider: None,
        model: None,
    }
}

fn prepared(request: &SessionTitleProviderRequest) -> SessionTitleLlmPreparedRequest {
    SessionTitleLlmPreparedRequest {
        system: "Exact provider system prompt".into(),
        input: "Exact provider user input".into(),
        message_seqs: request.messages.iter().map(|message| message.seq).collect(),
        select_reasoning_effort: Arc::new(|_| None),
    }
}

#[test]
fn validates_config_and_exposes_the_loader_schema() {
    let schema = serde_json::to_value(schema_for!(SessionTitleLlmConfig)).unwrap();
    let object = &schema;
    assert_eq!(
        object["required"],
        json!(["maxInputBytes", "maxOutputTokens", "timeoutMs"])
    );
    assert_eq!(object["additionalProperties"], false);
    assert_eq!(
        object["properties"]["timeoutMs"]["maximum"],
        MAX_TIMER_DELAY_MS
    );
    assert_eq!(object["properties"]["timeoutMs"]["multipleOf"], 1);
    assert_eq!(object["properties"]["provider"]["type"], "string");
    assert_eq!(object["properties"]["model"]["type"], "string");

    let cases = [
        (Value::Null, "session-title-llm: configuration is required"),
        (
            Value::String("invalid".into()),
            "session-title-llm: configuration is required",
        ),
        (
            json!([]),
            "session-title-llm: maxInputBytes must be a positive integer",
        ),
        (json!([1]), "session-title-llm: unknown config key \"0\""),
        (
            json!({}),
            "session-title-llm: maxInputBytes must be a positive integer",
        ),
        (
            json!({"maxInputBytes":1,"maxOutputTokens":1,"timeoutMs":1,"extra":true}),
            "session-title-llm: unknown config key \"extra\"",
        ),
        (
            json!({"maxInputBytes":1.5,"maxOutputTokens":1,"timeoutMs":1}),
            "session-title-llm: maxInputBytes must be a positive integer",
        ),
        (
            json!({"maxInputBytes":1,"maxOutputTokens":0,"timeoutMs":1}),
            "session-title-llm: maxOutputTokens must be a positive integer",
        ),
        (
            json!({"maxInputBytes":1,"maxOutputTokens":1,"timeoutMs":1.5}),
            "session-title-llm: timeoutMs must be a positive integer",
        ),
        (
            json!({"maxInputBytes":1,"maxOutputTokens":1,"timeoutMs":0}),
            "session-title-llm: timeoutMs must be a positive integer",
        ),
        (
            json!({"maxInputBytes":1,"maxOutputTokens":1,"timeoutMs":MAX_TIMER_DELAY_MS+1}),
            "session-title-llm: timeoutMs must not exceed 2147483647",
        ),
        (
            json!({"maxInputBytes":1,"maxOutputTokens":1,"timeoutMs":1e20}),
            "session-title-llm: timeoutMs must not exceed 2147483647",
        ),
        (
            json!({
                "maxInputBytes":1,
                "maxOutputTokens":1,
                "timeoutMs":1.8446744073709552e19
            }),
            "session-title-llm: timeoutMs must not exceed 2147483647",
        ),
        (
            json!({"maxInputBytes":1,"maxOutputTokens":1,"timeoutMs":1,"provider":"p"}),
            "session-title-llm: provider and model must be supplied together",
        ),
        (
            json!({"maxInputBytes":1,"maxOutputTokens":1,"timeoutMs":1,"model":"m"}),
            "session-title-llm: provider and model must be supplied together",
        ),
        (
            json!({"provider":"p"}),
            "session-title-llm: maxInputBytes must be a positive integer",
        ),
        (
            json!({"maxInputBytes":1,"maxOutputTokens":1,"timeoutMs":1,"provider":"","model":"m"}),
            "session-title-llm: provider and model overrides must be non-empty strings",
        ),
        (
            json!({"maxInputBytes":1,"maxOutputTokens":1,"timeoutMs":1,"provider":"p","model":""}),
            "session-title-llm: provider and model overrides must be non-empty strings",
        ),
        (
            json!({"maxInputBytes":1,"maxOutputTokens":1,"timeoutMs":1,"provider":1,"model":"m"}),
            "session-title-llm: provider and model overrides must be non-empty strings",
        ),
        (
            json!({"maxInputBytes":1,"maxOutputTokens":1,"timeoutMs":1,"provider":"p","model":1}),
            "session-title-llm: provider and model overrides must be non-empty strings",
        ),
    ];
    for (candidate, expected) in cases {
        assert_eq!(
            resolve_session_title_llm_config_value(&candidate)
                .unwrap_err()
                .to_string(),
            expected
        );
    }
    assert_eq!(
        resolve_session_title_llm_config_value(&json!({
            "maxInputBytes": 2,
            "maxOutputTokens": 3,
            "timeoutMs": 4,
            "provider": "p",
            "model": "m"
        }))
        .unwrap()
        .provider
        .as_deref(),
        Some("p")
    );

    let mut typed = config();
    typed.max_output_tokens = 1.5;
    assert_eq!(
        resolve_session_title_llm_config(&typed)
            .unwrap_err()
            .to_string(),
        "session-title-llm: maxOutputTokens must be a positive integer"
    );
    typed.max_output_tokens = 1.0;
    typed.provider = Some("provider".into());
    assert_eq!(
        resolve_session_title_llm_config(&typed)
            .unwrap_err()
            .to_string(),
        "session-title-llm: provider and model must be supplied together"
    );
    let mut huge_timeout = config();
    huge_timeout.timeout_ms = 1e20;
    assert_eq!(
        resolve_session_title_llm_config(&huge_timeout)
            .unwrap_err()
            .to_string(),
        "session-title-llm: timeoutMs must not exceed 2147483647"
    );

    for (field, value) in [
        ("maxInputBytes", f64::NAN),
        ("maxOutputTokens", f64::INFINITY),
        ("timeoutMs", -1.0),
    ] {
        let mut invalid = config();
        match field {
            "maxInputBytes" => invalid.max_input_bytes = value,
            "maxOutputTokens" => invalid.max_output_tokens = value,
            "timeoutMs" => invalid.timeout_ms = value,
            _ => unreachable!(),
        }
        assert_eq!(
            resolve_session_title_llm_config(&invalid)
                .unwrap_err()
                .to_string(),
            format!("session-title-llm: {field} must be a positive integer")
        );
    }
}

#[tokio::test]
async fn dispatches_and_logs_the_exact_request_before_streaming() {
    let adapter = RecordingAdapter::new(FinishReason::new("stop"), &["off", "high"]);
    let (kernel, ctx) = context(adapter.clone(), &["current-route"]).await;
    let controller = AbortController::new();
    let request = make_request(controller.signal());
    *adapter.session.lock().unwrap() = Some(request.session.clone());
    let mut prepared = prepared(&request);
    prepared.system = "Provider-owned system instruction".into();
    prepared.input = "Provider-owned user payload with 第二个问题".into();
    prepared.message_seqs = vec![SessionSeq::new(1).unwrap(), SessionSeq::new(2).unwrap()];
    prepared.select_reasoning_effort = Arc::new(|model| {
        model
            .reasoning
            .as_ref()
            .and_then(|reasoning| reasoning.efforts.first())
            .map(|effort| effort.id.clone())
    });
    let result = execute_session_title_llm(
        &ctx,
        &resolve_session_title_llm_config(&config()).unwrap(),
        &request,
        "test-title-provider".into(),
        &prepared,
    )
    .await
    .unwrap();

    assert_eq!(
        result.blocks,
        vec![hoh_llm::ContentBlock::text("  五个字标题  ")]
    );
    assert!(matches!(result.finish, SessionTitleLlmFinish::Stop(_)));
    assert_eq!(result.model.provider, "current-route");
    assert!(adapter.logged_at_dispatch.load(Ordering::SeqCst));
    let options = adapter.requests.lock().unwrap()[0].clone();
    assert_eq!(options.provider, "current-route");
    assert_eq!(options.model, "current-model");
    assert_eq!(options.max_tokens, Some(32));
    assert_eq!(
        options.reasoning_effort.as_ref().map(ToString::to_string),
        Some("off".into())
    );
    assert_eq!(options.system.as_deref(), Some(prepared.system.as_str()));
    assert_eq!(
        serde_json::to_value(&options.messages).unwrap()[0]["content"][0]["text"],
        prepared.input
    );
    assert_eq!(options.session_id.as_ref(), Some(request.session.id()));
    assert_eq!(options.purpose.as_deref(), Some("session-title"));
    assert!(!hoh_llm::is_agent_loop_request(&options));

    let event = request
        .session
        .snapshot_events(None, None)
        .iter()
        .find(|event| event.event_type() == "session/title-llm-request")
        .unwrap()
        .data()
        .clone();
    assert_eq!(
        event,
        json!({
            "titleProvider": "test-title-provider",
            "messageSeqs": [1, 2],
            "route": {"provider":"current-route","model":"current-model"},
            "system": prepared.system,
            "messages": options.messages,
            "maxTokens": 32,
            "reasoningEffort": "off"
        })
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn forwards_ceiling_reasoning_selector_and_keeps_title_cap_independent() {
    let adapter = RecordingAdapter::new(FinishReason::new("stop"), &["off", "low", "high", "max"]);
    let (kernel, ctx) = context(adapter.clone(), &["current-route"]).await;

    let selected = make_request(AbortController::new().signal());
    let mut selected_prepared = prepared(&selected);
    selected_prepared.select_reasoning_effort = Arc::new(|model| {
        model
            .reasoning
            .as_ref()
            .and_then(|reasoning| reasoning.efforts.last())
            .map(|effort| effort.id.clone())
    });
    execute_session_title_llm(
        &ctx,
        &resolve_session_title_llm_config(&config()).unwrap(),
        &selected,
        "provider".into(),
        &selected_prepared,
    )
    .await
    .unwrap();
    assert_eq!(
        adapter.requests.lock().unwrap()[0]
            .reasoning_effort
            .as_ref()
            .map(ToString::to_string),
        Some("max".into())
    );
    let selected_event = selected
        .session
        .snapshot_events(None, None)
        .iter()
        .find(|event| event.event_type() == "session/title-llm-request")
        .unwrap()
        .data()
        .clone();
    assert_eq!(selected_event["reasoningEffort"], "max");

    for conversation_cap in [8, 4_096] {
        let request = make_request_with_conversation_cap(
            AbortController::new().signal(),
            Some(conversation_cap),
        );
        execute_session_title_llm(
            &ctx,
            &resolve_session_title_llm_config(&config()).unwrap(),
            &request,
            "provider".into(),
            &prepared(&request),
        )
        .await
        .unwrap();
        assert_eq!(
            adapter.requests.lock().unwrap().last().unwrap().max_tokens,
            Some(32)
        );
        assert_eq!(
            adapter
                .requests
                .lock()
                .unwrap()
                .last()
                .unwrap()
                .reasoning_effort,
            None
        );
        let event = request
            .session
            .snapshot_events(None, None)
            .iter()
            .find(|event| event.event_type() == "session/title-llm-request")
            .unwrap()
            .data()
            .clone();
        assert_eq!(event["maxTokens"], 32);
        assert!(event.get("reasoningEffort").is_none());
    }
    kernel.shutdown().await;
}

#[tokio::test]
async fn applies_explicit_route_and_exact_utf8_input_bound_without_logging_rejections() {
    let adapter = RecordingAdapter::new(FinishReason::new("max-tokens"), &[]);
    let (kernel, ctx) = context(adapter.clone(), &["override-provider"]).await;
    let request = make_request(AbortController::new().signal());
    let input = "标题输入：\"番茄\"";
    let mut prepared = prepared(&request);
    prepared.input = input.into();
    let mut raw = config();
    raw.max_input_bytes = (input.len() - 1) as f64;
    raw.provider = Some("override-provider".into());
    raw.model = Some("override-model".into());
    let error = execute_session_title_llm(
        &ctx,
        &resolve_session_title_llm_config(&raw).unwrap(),
        &request,
        "provider".into(),
        &prepared,
    )
    .await
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        format!(
            "session-title-llm: input is {} bytes, exceeding maxInputBytes {}",
            input.len(),
            input.len() - 1
        )
    );
    assert!(adapter.requests.lock().unwrap().is_empty());
    assert!(request.session.snapshot_events(None, None).is_empty());

    raw.max_input_bytes = input.len() as f64;
    let result = execute_session_title_llm(
        &ctx,
        &resolve_session_title_llm_config(&raw).unwrap(),
        &request,
        "provider".into(),
        &prepared,
    )
    .await
    .unwrap();
    assert!(matches!(result.finish, SessionTitleLlmFinish::MaxTokens(_)));
    assert_eq!(
        result.blocks,
        vec![hoh_llm::ContentBlock::text("  五个字标题  ")]
    );
    assert_eq!(result.model.provider, "override-provider");
    assert_eq!(adapter.requests.lock().unwrap()[0].reasoning_effort, None);
    let event = request
        .session
        .snapshot_events(None, None)
        .iter()
        .find(|event| event.event_type() == "session/title-llm-request")
        .unwrap()
        .data()
        .clone();
    assert_eq!(
        event["route"],
        json!({"provider":"override-provider","model":"override-model"})
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn rejects_missing_attribution_route_abort_and_unregistered_route_before_dispatch() {
    let adapter = RecordingAdapter::new(FinishReason::new("stop"), &[]);
    let (kernel, ctx) = context(adapter.clone(), &["different-route"]).await;
    let controller = AbortController::new();
    let mut provider_request = make_request(controller.signal());
    let mut prepared = prepared(&provider_request);
    prepared.message_seqs.clear();
    assert_eq!(
        execute_session_title_llm(
            &ctx,
            &resolve_session_title_llm_config(&config()).unwrap(),
            &provider_request,
            "provider".into(),
            &prepared,
        )
        .await
        .unwrap_err()
        .to_string(),
        "session-title-llm: at least one source message is required"
    );
    prepared.message_seqs.push(SessionSeq::new(1).unwrap());
    provider_request.route = None;
    assert_eq!(
        execute_session_title_llm(
            &ctx,
            &resolve_session_title_llm_config(&config()).unwrap(),
            &provider_request,
            "provider".into(),
            &prepared,
        )
        .await
        .unwrap_err()
        .to_string(),
        "session-title-llm: no logged request route is available; configure provider and model together"
    );
    provider_request.route = Some(SessionTitleModelIdentity {
        provider: "current-route".into(),
        model: "current-model".into(),
    });
    let error = execute_session_title_llm(
        &ctx,
        &resolve_session_title_llm_config(&config()).unwrap(),
        &provider_request,
        "provider".into(),
        &prepared,
    )
    .await
    .unwrap_err();
    assert_eq!(
        HarnessError::of(error.as_ref()).unwrap().code(),
        "NO_ADAPTER"
    );
    assert!(
        provider_request
            .session
            .snapshot_events(None, None)
            .is_empty()
    );

    let aborted = AbortController::new();
    aborted.abort_with(AbortError::new("caller stopped"));
    let aborted_request = make_request(aborted.signal());
    assert_eq!(
        execute_session_title_llm(
            &ctx,
            &resolve_session_title_llm_config(&config()).unwrap(),
            &aborted_request,
            "provider".into(),
            &prepared,
        )
        .await
        .unwrap_err()
        .to_string(),
        "caller stopped"
    );
    assert!(adapter.requests.lock().unwrap().is_empty());
    kernel.shutdown().await;
}

#[tokio::test]
async fn preserves_error_and_aborted_finish_message_and_code() {
    for kind in ["error", "aborted"] {
        let adapter = RecordingAdapter::new(
            FinishReason::failed(kind, LlmFailure::new("model failed", "MODEL_FAILED")),
            &[],
        );
        let (kernel, ctx) = context(adapter, &["current-route"]).await;
        let request = make_request(AbortController::new().signal());
        let error = execute_session_title_llm(
            &ctx,
            &resolve_session_title_llm_config(&config()).unwrap(),
            &request,
            "provider".into(),
            &prepared(&request),
        )
        .await
        .unwrap_err();
        let failure = error.downcast_ref::<SessionTitleLlmFinishError>().unwrap();
        assert_eq!(failure.message, "model failed");
        assert_eq!(failure.code, "MODEL_FAILED");
        assert!(
            request
                .session
                .snapshot_events(None, None)
                .iter()
                .any(|event| event.event_type() == "session/title-llm-request")
        );
        kernel.shutdown().await;
    }
}

#[tokio::test]
async fn returns_tool_call_blocks_and_rejects_an_unknown_finish_variant() {
    let tool_script = [
        json!({"type":"block-start","index":0,"blockType":"text"}),
        json!({"type":"text-delta","index":0,"text":"text beside a tool call"}),
        json!({"type":"block-start","index":1,"blockType":"tool-call"}),
        json!({"type":"tool-call-delta","index":1,"id":"title-tool","name":"unexpected","argumentsDelta":"{}"}),
        json!({"type":"finish","reason":{"kind":"tool-calls"}}),
    ]
    .into_iter()
    .map(|value| serde_json::from_value(value).unwrap())
    .collect();
    let adapter = RecordingAdapter::with_script(tool_script, &[]);
    let (kernel, ctx) = context(adapter, &["current-route"]).await;
    let request = make_request(AbortController::new().signal());
    let result = execute_session_title_llm(
        &ctx,
        &resolve_session_title_llm_config(&config()).unwrap(),
        &request,
        "provider".into(),
        &prepared(&request),
    )
    .await
    .unwrap();
    assert!(matches!(result.finish, SessionTitleLlmFinish::ToolCalls(_)));
    assert_eq!(
        serde_json::to_value(&result.blocks).unwrap(),
        json!([
            {"type":"text","text":"text beside a tool call"},
            {"type":"tool-call","id":"title-tool","name":"unexpected","arguments":"{}"}
        ])
    );
    kernel.shutdown().await;

    let adapter = RecordingAdapter::new(FinishReason::new("future-kind"), &[]);
    let (kernel, ctx) = context(adapter, &["current-route"]).await;
    let request = make_request(AbortController::new().signal());
    assert_eq!(
        execute_session_title_llm(
            &ctx,
            &resolve_session_title_llm_config(&config()).unwrap(),
            &request,
            "provider".into(),
            &prepared(&request),
        )
        .await
        .unwrap_err()
        .to_string(),
        "unreachable variant in FinishReason: {\"kind\":\"future-kind\"}"
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn aborts_a_cooperative_stream_for_deadline_and_caller_cancellation() {
    let (kernel, ctx) = context(Arc::new(CooperativeAdapter), &["current-route"]).await;
    let request = make_request(AbortController::new().signal());
    let mut raw = config();
    raw.timeout_ms = 10.0;
    let error = execute_session_title_llm(
        &ctx,
        &resolve_session_title_llm_config(&raw).unwrap(),
        &request,
        "provider".into(),
        &prepared(&request),
    )
    .await
    .unwrap_err();
    let reason = hoh_error::SharedError::of(error.as_ref()).unwrap();
    assert_eq!(
        timeout_of(Some(reason), Some(SESSION_TITLE_TIMEOUT_CODE)).unwrap(),
        &TimeoutReason::new(SESSION_TITLE_TIMEOUT_CODE, 10.0)
    );
    kernel.shutdown().await;

    let started = Arc::new(Notify::new());
    let adapter = RecordingAdapter::new(FinishReason::new("stop"), &[]);
    let (kernel, ctx) = context(adapter, &["current-route"]).await;
    let listener_started = started.clone();
    ctx.on_waterfall::<LlmStreamEvent, _, _>(move |event, _| {
        let signal = event.options().signal.expect("title request signal");
        let listener_started = listener_started.clone();
        async move {
            Ok(LlmStream::new(stream::once(async move {
                listener_started.notify_one();
                signal.cancelled().await;
                Ok::<StreamChunk, BoxError>(StreamChunk::finish(FinishReason::new("stop")))
            }))
            .with_cleanup(|| -> LlmFuture<'static, ()> {
                Box::pin(async { Err("close failed".into()) })
            }))
        }
    })
    .unwrap();
    let controller = AbortController::new();
    let request = make_request(controller.signal());
    let resolved = resolve_session_title_llm_config(&config()).unwrap();
    let prepared = prepared(&request);
    let execution =
        execute_session_title_llm(&ctx, &resolved, &request, "provider".into(), &prepared);
    let cancellation = async {
        started.notified().await;
        controller.abort_with(AbortError::new("caller stopped"));
    };
    let (result, ()) = tokio::join!(execution, cancellation);
    assert_eq!(result.unwrap_err().to_string(), "caller stopped");
    kernel.shutdown().await;
}

#[tokio::test]
async fn stream_error_precedes_an_already_expired_deadline_and_preserves_its_code() {
    let adapter = RecordingAdapter::new(FinishReason::new("stop"), &[]);
    let (kernel, ctx) = context(adapter, &["current-route"]).await;
    ctx.on_waterfall::<LlmStreamEvent, _, _>(|_, _| async {
        Ok(LlmStream::new(stream::once(async {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            Err(Box::new(SessionTitleLlmFinishError {
                message: "connection reset".into(),
                code: "TRANSPORT_RESET".into(),
            }) as BoxError)
        }))
        .with_cleanup(|| -> LlmFuture<'static, ()> {
            Box::pin(async { Err("close failed".into()) })
        }))
    })
    .unwrap();
    let request = make_request(AbortController::new().signal());
    let mut raw = config();
    raw.timeout_ms = 10.0;
    let error = execute_session_title_llm(
        &ctx,
        &resolve_session_title_llm_config(&raw).unwrap(),
        &request,
        "provider".into(),
        &prepared(&request),
    )
    .await
    .unwrap_err();
    let failure = error.downcast_ref::<SessionTitleLlmFinishError>().unwrap();
    assert_eq!(failure.message, "connection reset");
    assert_eq!(failure.code, "TRANSPORT_RESET");
    kernel.shutdown().await;
}

#[tokio::test]
async fn rejects_a_late_success_at_the_configured_deadline() {
    let (kernel, ctx) = context(
        Arc::new(DelayedAdapter { delay_ms: 20 }),
        &["current-route"],
    )
    .await;
    let request = make_request(AbortController::new().signal());
    let mut raw = config();
    raw.timeout_ms = 10.0;
    let resolved = resolve_session_title_llm_config(&raw).unwrap();
    let prepared = prepared(&request);
    let future = execute_session_title_llm(&ctx, &resolved, &request, "provider".into(), &prepared);
    let error = future.await.unwrap_err();
    let reason = hoh_error::SharedError::of(error.as_ref()).unwrap();
    let timeout = timeout_of(Some(reason), Some(SESSION_TITLE_TIMEOUT_CODE)).unwrap();
    assert_eq!(
        timeout,
        &TimeoutReason::new(SESSION_TITLE_TIMEOUT_CODE, 10.0)
    );
    assert!(
        request
            .session
            .snapshot_events(None, None)
            .iter()
            .any(|event| event.event_type() == "session/title-llm-request")
    );
    kernel.shutdown().await;
}
