mod common;
use common::*;
use futures_util::StreamExt;
use hoh_error::{BoxError, SharedError};
use hoh_kernel::Kernel;
use hoh_llm::*;
use hoh_timeout::{AbortController, AbortSignal};
use serde_json::json;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
fn code<'a>(error: &'a (dyn std::error::Error + 'static)) -> &'a str {
    HarnessError::of(error).unwrap().code()
}
#[test]
fn recognizes_structured_and_model_capacity_context_window_overflow_details() {
    for s in [
        "context_length_exceeded maximum context length",
        "context-window-overflowed",
        "This model maximum context length is 128000 tokens",
        "input is too long for this model",
        "request too large for model context",
        "input exceeds the model context window limit",
    ] {
        assert!(is_context_window_exceeded_error(s), "{s}");
    }
}
#[test]
fn does_not_mistake_unrelated_input_validation_for_context_window_overflow() {
    for s in [
        "invalid request: malformed tool arguments",
        "invalid input: temperature exceeds maximum allowed value",
        "input exceeds maximum allowed value",
        "context window size must be positive",
    ] {
        assert!(!is_context_window_exceeded_error(s));
    }
}
#[test]
fn distinguishes_exhausted_account_quota_from_transient_rate_limiting() {
    for s in [
        "insufficient_quota",
        "account balance depleted",
        "usage-limit-exceeded",
        "out of credits",
        "OpenAI API error (429): You exceeded your current quota, please check your plan and billing details.",
    ] {
        assert!(is_quota_exceeded_error(s));
    }
    for s in ["HTTP 429: rate limit reached", "quota resets in one minute"] {
        assert!(!is_quota_exceeded_error(s));
    }
}
#[test]
fn error_chain_renders_the_full_cause_chain_of_a_wrapped_transport_failure() {
    let e = HarnessError::new("fetch failed", "UNKNOWN").with_cause(ValueError::Message(
        "connect ECONNREFUSED 127.0.0.1:443".into(),
    ));
    assert_eq!(
        error_chain(&e),
        "fetch failed: connect ECONNREFUSED 127.0.0.1:443"
    );
}
#[test]
fn error_chain_renders_aggregate_error_members() {
    let e = HarnessError::new("fetch failed", "UNKNOWN").with_cause(AggregateError {
        message: "".into(),
        errors: vec![
            SharedError::new(ValueError::Message("connect ECONNREFUSED ::1:443".into())),
            SharedError::new(ValueError::Message(
                "connect ECONNREFUSED 127.0.0.1:443".into(),
            )),
        ],
    });
    assert_eq!(
        error_chain(&e),
        "fetch failed: AggregateError [connect ECONNREFUSED ::1:443; connect ECONNREFUSED 127.0.0.1:443]"
    );
}
#[derive(Debug)]
struct Cycle;
impl std::fmt::Display for Cycle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("outer")
    }
}
impl std::error::Error for Cycle {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self)
    }
}
#[derive(Debug)]
struct Hostile;
impl std::fmt::Display for Hostile {
    fn fmt(&self, _: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        panic!("hostile getter")
    }
}
impl std::error::Error for Hostile {}
#[test]
fn error_chain_survives_non_error_values_hostile_coercion_and_circular_causes() {
    assert_eq!(error_chain_value(&json!("plain string")), "plain string");
    assert_eq!(
        error_chain_value(&json!({"message":"structured provider failure","code":"SERVER"})),
        "structured provider failure"
    );
    assert_eq!(error_chain(&Cycle), "outer: <circular cause>");
    assert_eq!(
        error_chain(&HarnessError::new("outer", "UNKNOWN").with_cause(Hostile)),
        "outer: <unrenderable value>"
    );
    let shared = SharedError::new(ValueError::Message("shared".into()));
    let e = AggregateError {
        message: "agg".into(),
        errors: vec![
            SharedError::new(HarnessError::new("a", "X").with_cause(shared.clone())),
            SharedError::new(HarnessError::new("b", "X").with_cause(shared)),
        ],
    };
    assert_eq!(error_chain(&e), "agg [a: shared; b: shared]");
}
#[test]
fn error_chain_falls_back_to_the_error_name_skips_empty_aggregates_and_stops_at_null_causes() {
    assert_eq!(
        error_chain(&HarnessError::named("TypeError", "", "UNKNOWN")),
        "TypeError"
    );
    assert_eq!(
        error_chain(&AggregateError {
            message: "all failed".into(),
            errors: vec![]
        }),
        "all failed"
    );
}
#[test]
fn error_chain_collapses_a_cause_that_repeats_the_wrapper_message_verbatim() {
    assert_eq!(
        error_chain(
            &HarnessError::new("boom", "UNKNOWN").with_cause(ValueError::Message("boom".into()))
        ),
        "boom"
    );
}
#[tokio::test]
async fn routes_stream_to_the_registered_adapter() {
    let (k, c, r, _) = setup().await;
    assert_eq!(collect(r.stream(&c, options())).await.unwrap(), script());
    k.shutdown().await;
}
#[tokio::test]
async fn sends_complete_declarations_without_deferred_loading_or_developer_updates_on_an_undeclared_route()
 {
    let (k, c, r, a) = setup().await;
    let added = developer(vec![
        ToolAdditionBlock {
            tool_name: "search".into(),
        }
        .into(),
    ]);
    let mut o = options();
    o.messages = vec![user(vec![ContentBlock::text("hi")]), added.clone()];
    o.tools = Some(vec![ToolSchema {
        defer_loading: Some(true),
        ..tool("search")
    }]);
    o.tool_history = Some(ToolHistory {
        tools: vec![],
        updates: vec![update(&added, vec![tool("search")])],
    });
    collect(r.stream(&c, o)).await.unwrap();
    let seen = a.seen.lock().unwrap().clone();
    assert_eq!(seen[0].tools.as_ref().unwrap(), &[tool("search")]);
    assert_eq!(seen[0].messages.len(), 1);
    drop(seen);
    k.shutdown().await;
}
#[tokio::test]
async fn projects_additions_and_removals_before_adapter_dispatch() {
    for mode in ["in-history", "addition-only"] {
        let (k, c, r, a) = setup().await;
        let mut info = metadata();
        info.tool_update = Some(mode.into());
        *a.metadata.lock().unwrap() = Some(info);
        let added = developer(vec![
            ToolAdditionBlock {
                tool_name: "search".into(),
            }
            .into(),
        ]);
        let removed = developer(vec![
            ToolRemovalBlock {
                tool_name: "search".into(),
            }
            .into(),
        ]);
        for removal in [false, true] {
            let prepared = r.prepare_call(&c, config(), None, None).await.unwrap();
            let mut o = options().with_call_config(&prepared.config);
            o.messages = vec![added.clone()];
            let mut updates = vec![update(&added, vec![tool("search")])];
            if removal {
                o.messages.push(removed.clone());
                updates.push(update(&removed, vec![]));
            }
            o.tools = Some(if removal {
                vec![]
            } else {
                vec![tool("search")]
            });
            o.tool_history = Some(ToolHistory {
                tools: vec![],
                updates,
            });
            collect(prepared.stream(o).unwrap()).await.unwrap();
            let seen = a.seen.lock().unwrap().clone();
            let last = seen.last().unwrap();
            if mode == "in-history" {
                assert_eq!(last.tools.as_ref().unwrap()[0].defer_loading, Some(true));
                assert_eq!(last.messages.len(), if removal { 2 } else { 1 });
            } else {
                assert!(last.messages.is_empty());
                assert_eq!(last.tools.as_ref().unwrap().len(), usize::from(!removal));
            }
        }
        k.shutdown().await;
    }
}
#[tokio::test]
async fn trusts_the_immutable_message_creation_boundary_for_direct_calls() {
    let (k, c, r, a) = setup().await;
    let m = user(vec![ContentBlock::text("hello")]);
    let mut o = options();
    o.messages = vec![m.clone()];
    collect(r.stream(&c, o)).await.unwrap();
    let seen = a.seen.lock().unwrap().clone();
    match (&seen[0].messages[0], &m) {
        (RequestMessage::Durable(a), RequestMessage::Durable(b)) => assert!(Arc::ptr_eq(a, b)),
        _ => panic!("durable message"),
    };
    drop(seen);
    k.shutdown().await;
}
#[tokio::test]
async fn projects_file_blocks_through_every_host_path_availability_outcome() {
    for case in 0..6 {
        let (k, c, r, a) = setup().await;
        let c = if case < 2 {
            c.with_tag(HostPathMapping(Arc::new(move |_| {
                Some(if case == 0 {
                    "/sandbox/notes.pdf".into()
                } else {
                    "/code-sandbox/notes.pdf".into()
                })
            })))
        } else {
            c
        };
        if case != 2 {
            let host = match case {
                3 => Ok(None),
                4 => Err(HarnessError::new("invalid ref", "INVALID")),
                _ => Ok(Some("/host/notes.pdf".into())),
            };
            c.provide::<hoh_attachment::Attachments>(Arc::new(TestAttachments::new(host)))
                .await
                .unwrap();
        }
        let mut o = options();
        o.messages = vec![user(vec![file()])];
        collect(r.stream(&c, o)).await.unwrap();
        let seen = a.seen.lock().unwrap().clone();
        let text = seen[0].messages[0].content()[0].text_value().unwrap();
        assert!(text.contains(if case == 0 {
            "\"/sandbox/notes.pdf\""
        } else if case == 1 {
            "\"/code-sandbox/notes.pdf\""
        } else {
            "current execution environment cannot access a readable path"
        }));
        assert_eq!(
            r.file_request_text(&c, &file().file().unwrap().attachment),
            text
        );
        drop(seen);
        k.shutdown().await;
    }
}
#[tokio::test]
async fn captures_provider_owned_retry_policy_at_registration_and_defaults_omission() {
    let k = Kernel::start();
    let c = k.root();
    let r = LlmRuntime::new();
    let a = Arc::new(RecordingAdapter::default());
    let policy = resolve_retry_policy_value(Some(&json!({"mode":"always"})), "p").unwrap();
    *a.policy.lock().unwrap() = Some(policy.clone());
    r.register_adapter(&c, vec!["configured".into()], a.clone())
        .unwrap();
    *a.policy.lock().unwrap() = None;
    r.register_adapter(&c, vec!["defaulted".into()], a).unwrap();
    assert_eq!(r.provider_retry_policy("configured").unwrap(), policy);
    assert_eq!(
        value(r.provider_retry_policy("defaulted").unwrap())["maxRetries"],
        5
    );
    assert_eq!(
        r.provider_retry_policy("missing").unwrap_err().code(),
        "NO_ADAPTER"
    );
    k.shutdown().await;
}
struct ThrowingAdapter {
    failure: HarnessError,
    iteration: bool,
}
impl LlmAdapter for ThrowingAdapter {
    fn stream(&self, _: GenerateOptions) -> Result<LlmStream, BoxError> {
        let e = self.failure.clone();
        if self.iteration {
            Ok(LlmStream::new(futures_util::stream::once(async move {
                Err(e.into())
            })))
        } else {
            Err(e.into())
        }
    }
}
#[tokio::test]
async fn keeps_a_prepared_registration_and_retry_policy_after_route_replacement() {
    let k = Kernel::start();
    let c = k.root();
    let r = LlmRuntime::new();
    let old = Arc::new(RecordingAdapter::default());
    let old_policy = resolve_retry_policy_value(Some(&json!({"mode":"always"})), "p").unwrap();
    *old.policy.lock().unwrap() = Some(old_policy.clone());
    let h = r
        .register_adapter(&c, vec!["route".into()], old.clone())
        .unwrap();
    let p = r.prepare_call(&c, config(), None, None).await.unwrap();
    h.dispose();
    let new = Arc::new(RecordingAdapter::default());
    let new_policy =
        resolve_retry_policy_value(Some(&json!({"mode":"normal","maxRetries":0})), "p").unwrap();
    *new.policy.lock().unwrap() = Some(new_policy.clone());
    r.register_adapter(&c, vec!["route".into()], new.clone())
        .unwrap();
    collect(p.stream(options()).unwrap()).await.unwrap();
    assert_eq!(old.seen.lock().unwrap().len(), 1);
    assert!(new.seen.lock().unwrap().is_empty());
    assert_eq!(p.retry_policy, old_policy);
    assert_eq!(r.provider_retry_policy("route").unwrap(), new_policy);
    k.shutdown().await;
}
#[tokio::test]
async fn normalizes_an_unregistered_provider_to_a_terminal_failure() {
    let k = Kernel::start();
    let c = k.root();
    let r = LlmRuntime::new();
    let chunks = collect(r.stream(&c, options())).await.unwrap();
    assert_eq!(chunks.len(), 1);
    let f = match &chunks[0] {
        StreamChunk::Finish { reason, .. } => reason.failure().unwrap(),
        _ => panic!("finish"),
    };
    assert_eq!(f.code, "NO_ADAPTER");
    assert!(f.message.contains("no adapter registered"));
    k.shutdown().await;
}
#[tokio::test]
async fn normalizes_a_throwing_iterator_result() {
    let k = Kernel::start();
    let c = k.root();
    let r = LlmRuntime::new();
    r.register_adapter(
        &c,
        vec!["route".into()],
        Arc::new(ThrowingAdapter {
            failure: LlmError::from_failure(
                &LlmFailure::new("value getter failed", "RESULT_GETTER_FAILED"),
                None,
            )
            .unwrap(),
            iteration: true,
        }),
    )
    .unwrap();
    let out = collect(r.stream(&c, options())).await.unwrap();
    assert_eq!(
        value(out)[0]["reason"]["failure"]["code"],
        "RESULT_GETTER_FAILED"
    );
    k.shutdown().await;
}
#[tokio::test]
async fn normalizes_synchronous_adapter_dispatch_failures() {
    let k = Kernel::start();
    let c = k.root();
    let r = LlmRuntime::new();
    r.register_adapter(
        &c,
        vec!["route".into()],
        Arc::new(ThrowingAdapter {
            failure: LlmError::from_failure(
                &LlmFailure::new("dispatch failed", "BOUNDARY_FAILED"),
                None,
            )
            .unwrap(),
            iteration: false,
        }),
    )
    .unwrap();
    let out = collect(r.stream(&c, options())).await.unwrap();
    assert_eq!(
        value(out)[0]["reason"]["failure"]["code"],
        "BOUNDARY_FAILED"
    );
    k.shutdown().await;
}
#[tokio::test]
async fn preserves_structured_llm_error_facts_in_the_terminal_failure() {
    let k = Kernel::start();
    let c = k.root();
    let r = LlmRuntime::new();
    let f = LlmError::with_options(
        "provider busy",
        BuiltinLlmFailureCode::RateLimit,
        LlmErrorOptions {
            status: Some(429),
            provider_retry_after_ms: Some(1500.0),
            request_id: Some("req-7".into()),
            ..Default::default()
        },
    )
    .unwrap();
    r.register_adapter(
        &c,
        vec!["route".into()],
        Arc::new(ThrowingAdapter {
            failure: f,
            iteration: false,
        }),
    )
    .unwrap();
    assert_eq!(
        value(collect(r.stream(&c, options())).await.unwrap()),
        json!([{"type":"finish","reason":{"kind":"error","failure":{"message":"provider busy","code":"RATE_LIMIT","status":429,"providerRetryAfterMs":1500,"requestId":"req-7"}}}])
    );
    k.shutdown().await;
}
struct PlainThrowing;
impl LlmAdapter for PlainThrowing {
    fn stream(&self, _: GenerateOptions) -> Result<LlmStream, BoxError> {
        Ok(LlmStream::new(futures_util::stream::once(async {
            Err(ValueError::Message("plain provider failure".into()).into())
        })))
    }
}
#[tokio::test]
async fn normalizes_arbitrary_adapter_rejections_without_throwing_them_downstream() {
    let k = Kernel::start();
    let c = k.root();
    let r = LlmRuntime::new();
    r.register_adapter(&c, vec!["route".into()], Arc::new(PlainThrowing))
        .unwrap();
    assert_eq!(
        value(collect(r.stream(&c, options())).await.unwrap())[0]["reason"]["failure"],
        json!({"message":"plain provider failure","code":"UNKNOWN"})
    );
    k.shutdown().await;
}
#[tokio::test]
async fn maps_adapter_failure_to_aborted_when_the_request_signal_is_aborted() {
    let k = Kernel::start();
    let c = k.root();
    let r = LlmRuntime::new();
    r.register_adapter(
        &c,
        vec!["route".into()],
        Arc::new(ThrowingAdapter {
            failure: LlmError::new("stopped", BuiltinLlmFailureCode::Unknown).unwrap(),
            iteration: false,
        }),
    )
    .unwrap();
    let abort = AbortController::new();
    abort.abort();
    let o = GenerateOptions {
        signal: Some(abort.signal()),
        ..options()
    };
    assert_eq!(
        value(collect(r.stream(&c, o)).await.unwrap())[0]["reason"]["kind"],
        "aborted"
    );
    k.shutdown().await;
}
#[tokio::test]
async fn leaves_middleware_and_consumer_failures_thrown() {
    let (k, c, r, _) = setup().await;
    c.on_waterfall::<LlmStreamEvent, _, _>(|_, _| async {
        Ok(LlmStream::new(futures_util::stream::once(async {
            Err(ValueError::Message("middleware failed".into()).into())
        })))
    })
    .unwrap();
    assert_eq!(
        collect(r.stream(&c, options()))
            .await
            .unwrap_err()
            .to_string(),
        "middleware failed"
    );
    let failure: Result<(), BoxError> = async {
        let mut stream = r.stream(&c, options());
        if stream.next().await.is_some() {
            return Err(ValueError::Message("consumer failed".into()).into());
        }
        Ok(())
    }
    .await;
    assert_eq!(failure.unwrap_err().to_string(), "consumer failed");
    k.shutdown().await;
}
struct CleanupAdapter(Arc<AtomicUsize>);
impl LlmAdapter for CleanupAdapter {
    fn stream(&self, _: GenerateOptions) -> Result<LlmStream, BoxError> {
        let count = self.0.clone();
        Ok(LlmStream::chunks(script()).with_cleanup(move || {
            Box::pin(async move {
                count.fetch_add(1, Ordering::SeqCst);
                Err(ValueError::Message("cleanup failed".into()).into())
            })
        }))
    }
}
#[tokio::test]
async fn awaits_adapter_cleanup_on_downstream_close_and_leaves_cleanup_failure_thrown() {
    let k = Kernel::start();
    let c = k.root();
    let r = LlmRuntime::new();
    let count = Arc::new(AtomicUsize::new(0));
    r.register_adapter(
        &c,
        vec!["route".into()],
        Arc::new(CleanupAdapter(count.clone())),
    )
    .unwrap();
    let mut stream = r.stream(&c, options());
    stream.next().await.unwrap().unwrap();
    assert_eq!(
        stream.close().await.unwrap_err().to_string(),
        "cleanup failed"
    );
    assert_eq!(count.load(Ordering::SeqCst), 1);
    k.shutdown().await;
}
#[tokio::test]
async fn allows_downstream_close_when_an_adapter_iterator_has_no_return_method() {
    let (k, c, r, _) = setup().await;
    let mut stream = r.stream(&c, options());
    stream.next().await.unwrap().unwrap();
    stream.close().await.unwrap();
    k.shutdown().await;
}
#[tokio::test]
async fn unregisters_adapters_when_the_owning_fiber_is_disposed() {
    let k = Kernel::start();
    let c = k.root();
    let r = LlmRuntime::new();
    let child = c.child("adapter owner").unwrap();
    r.register_adapter(
        &child,
        vec!["scoped-model".into()],
        Arc::new(RecordingAdapter::default()),
    )
    .unwrap();
    assert_eq!(r.list_providers().len(), 1);
    child.fiber().dispose().await;
    assert!(r.list_providers().is_empty());
    k.shutdown().await;
}
#[tokio::test]
async fn discovers_detached_provider_and_advisory_model_metadata() {
    let (k, _, r, a) = setup().await;
    *a.models.lock().unwrap() = vec![decode(
        json!({"provider":"route","id":"fast","name":"Fast","description":"Low latency","inputModalities":["text"]}),
    )];
    let mut providers = r.list_providers();
    let mut models = r.list_models("route").await.unwrap();
    providers[0].name = "mutated".into();
    models[0].name = "mutated".into();
    assert_eq!(r.list_providers()[0].name, "route");
    assert_eq!(r.list_models("route").await.unwrap()[0].name, "Fast");
    k.shutdown().await;
}
#[tokio::test]
async fn detaches_advertised_model_badges_including_their_translations() {
    let (k, _, r, a) = setup().await;
    *a.models.lock().unwrap() = decode(
        json!([{"provider":"route","id":"new","name":"New","badge":{"text":{"en":"New","zh":"新"},"tone":"info"}},{"provider":"route","id":"dated","name":"Dated","badge":{"text":"1017","tone":"neutral"}}]),
    );
    let mut models = r.list_models("route").await.unwrap();
    assert_eq!(value(&models)[0]["badge"]["text"]["zh"], "新");
    models[0].badge = None;
    assert!(r.list_models("route").await.unwrap()[0].badge.is_some());
    k.shutdown().await;
}
#[tokio::test]
async fn defaults_adapters_to_their_route_name_and_an_empty_advisory_model_list() {
    let (k, _, r, _) = setup().await;
    assert_eq!(r.list_providers()[0].name, "route");
    assert!(r.list_models("route").await.unwrap().is_empty());
    assert_eq!(
        value(
            r.resolve_model_info("route", "unlisted", None)
                .await
                .unwrap()
        ),
        json!({"provider":"route","id":"unlisted","name":"unlisted"})
    );
    assert_eq!(
        code(r.list_models("missing").await.unwrap_err().as_ref()),
        "NO_ADAPTER"
    );
    assert_eq!(
        code(
            r.resolve_model_info("missing", "m", None)
                .await
                .unwrap_err()
                .as_ref()
        ),
        "NO_ADAPTER"
    );
    k.shutdown().await;
}
#[test]
fn rejects_invalid_exact_model_metadata() {
    for patch in [
        json!({"provider":"other"}),
        json!({"id":"other"}),
        json!({"name":""}),
        json!({"toolUpdate":"sometimes"}),
    ] {
        let mut v = value(metadata());
        for (k, vv) in patch.as_object().unwrap() {
            v[k] = vv.clone();
        }
        assert_eq!(
            normalize_model_info("route", "model", decode(v))
                .unwrap_err()
                .code(),
            "INVALID_MODEL_INFO"
        );
    }
    for patch in [
        json!({"provider":1}),
        json!({"id":1}),
        json!({"name":1}),
        json!({"description":1}),
    ] {
        let mut v = value(metadata());
        for (k, vv) in patch.as_object().unwrap() {
            v[k] = vv.clone();
        }
        assert!(serde_json::from_value::<LlmResolvedModelInfo>(v).is_err());
    }
}
#[tokio::test]
async fn preserves_modality_metadata_through_exact_model_resolution() {
    let (k, c, r, a) = setup().await;
    let mut info = metadata();
    info.model.input_modalities = Some(vec!["text".into(), "image".into()]);
    info.tool_update = Some("in-history".into());
    *a.metadata.lock().unwrap() = Some(info);
    let resolved = r.resolve_model_info("route", "model", None).await.unwrap();
    assert_eq!(
        resolved.model.input_modalities.as_ref().unwrap(),
        &["text", "image"]
    );
    let p = r.prepare_call(&c, config(), None, None).await.unwrap();
    assert_eq!(p.tool_update.as_deref(), Some("in-history"));
    k.shutdown().await;
}
#[tokio::test]
async fn resolves_detached_model_context_independently_of_advisory_catalog_membership() {
    let (k, _, r, a) = setup().await;
    let mut info = metadata();
    info.context = Some(LlmModelContext {
        context_window: 32000,
    });
    *a.metadata.lock().unwrap() = Some(info.clone());
    let resolved = r.resolve_model_info("route", "model", None).await.unwrap();
    a.metadata
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .context
        .as_mut()
        .unwrap()
        .context_window = 64000;
    assert_eq!(resolved.context.unwrap().context_window, 32000);
    assert!(r.list_models("route").await.unwrap().is_empty());
    k.shutdown().await;
}
fn reasoning_info(default: bool) -> LlmResolvedModelInfo {
    let mut info = metadata();
    info.reasoning = Some(decode(
        json!({"efforts":[{"id":"standard","name":"Standard"},{"id":"ultra","name":"Ultra","description":"Largest budget"}]}),
    ));
    if default {
        info.reasoning.as_mut().unwrap().default_effort = Some("standard".into());
    }
    info
}
#[tokio::test]
async fn resolves_detached_adapter_owned_reasoning_metadata_and_materializes_its_default() {
    let (k, _, r, a) = setup().await;
    *a.metadata.lock().unwrap() = Some(reasoning_info(true));
    let resolved = r.resolve_model_info("route", "model", None).await.unwrap();
    a.metadata
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .reasoning
        .as_mut()
        .unwrap()
        .efforts[0]
        .name = "mutated".into();
    assert_eq!(resolved.reasoning.unwrap().efforts[0].name, "Standard");
    assert_eq!(
        r.resolve_call_config(config(), None)
            .await
            .unwrap()
            .controls
            .reasoning_effort
            .as_ref()
            .map(|id| id.as_str()),
        Some("standard")
    );
    let mut explicit = config();
    explicit.controls.reasoning_effort = Some("ultra".into());
    assert_eq!(
        r.resolve_call_config(explicit.clone(), None).await.unwrap(),
        explicit
    );
    *a.metadata.lock().unwrap() = Some(reasoning_info(false));
    assert_eq!(
        r.resolve_call_config(config(), None).await.unwrap(),
        config()
    );
    k.shutdown().await;
}
#[tokio::test]
async fn materializes_an_adapter_owned_max_tokens_default_while_preserving_an_explicit_cap() {
    let (k, c, r, a) = setup().await;
    let mut info = metadata();
    info.default_max_tokens = Some(256000);
    *a.metadata.lock().unwrap() = Some(info);
    assert_eq!(
        r.resolve_call_config(config(), None)
            .await
            .unwrap()
            .controls
            .max_tokens,
        Some(256000)
    );
    let p = r.prepare_call(&c, config(), None, None).await.unwrap();
    assert_eq!(p.adapter_defaults.max_tokens, Some(true));
    let mut explicit = config();
    explicit.controls.max_tokens = Some(8192);
    assert_eq!(
        r.resolve_call_config(explicit.clone(), None).await.unwrap(),
        explicit
    );
    assert_eq!(
        r.prepare_call(&c, explicit, None, None)
            .await
            .unwrap()
            .adapter_defaults
            .max_tokens,
        None
    );
    k.shutdown().await;
}
#[test]
fn rejects_invalid_adapter_owned_default_max_tokens() {
    for n in [0, 9007199254740992_u64] {
        let mut info = metadata();
        info.default_max_tokens = Some(n);
        assert_eq!(
            normalize_model_info("route", "model", info)
                .unwrap_err()
                .code(),
            "INVALID_MODEL_MAX_TOKENS"
        );
    }
    let mut info = value(metadata());
    info["defaultMaxTokens"] = json!(1.5);
    assert!(serde_json::from_value::<LlmResolvedModelInfo>(info).is_err());
}
#[test]
fn rejects_invalid_model_reasoning_metadata() {
    for v in [
        json!({"efforts":[]}),
        json!({"efforts":[{"id":"","name":"Empty"}]}),
        json!({"efforts":[{"id":"valid","name":""}]}),
        json!({"efforts":[{"id":"same","name":"One"},{"id":"same","name":"Two"}]}),
        json!({"efforts":[{"id":"valid","name":"Valid"}],"defaultEffort":"other"}),
    ] {
        let mut info = metadata();
        info.reasoning = Some(decode(v));
        assert_eq!(
            normalize_model_info("route", "model", info)
                .unwrap_err()
                .code(),
            "INVALID_MODEL_REASONING"
        );
    }
    assert!(
        serde_json::from_value::<LlmModelReasoningInfo>(
            json!({"efforts":[{"id":"valid","name":"Valid","description":1}]})
        )
        .is_err()
    );
}
#[tokio::test]
async fn rejects_unsupported_reasoning_efforts_without_clamping() {
    let (k, _, r, a) = setup().await;
    let mut explicit = config();
    explicit.controls.reasoning_effort = Some("unsupported".into());
    for info in [metadata(), reasoning_info(false)] {
        *a.metadata.lock().unwrap() = Some(info);
        assert_eq!(
            code(
                r.resolve_call_config(explicit.clone(), None)
                    .await
                    .unwrap_err()
                    .as_ref()
            ),
            "UNSUPPORTED_REASONING_EFFORT"
        );
    }
    k.shutdown().await;
}
#[tokio::test]
async fn resolves_reasoning_defaults_at_the_final_adapter_boundary_after_routing_middleware() {
    let (k, c, r, a) = setup().await;
    *a.metadata.lock().unwrap() = Some(reasoning_info(true));
    c.on_waterfall::<LlmStreamEvent, _, _>(|event, next| async move {
        event.update_options(|o| o.provider = "route".into())?;
        next.run().await
    })
    .unwrap();
    let mut o = options();
    o.provider = "initial".into();
    collect(r.stream(&c, o)).await.unwrap();
    assert_eq!(
        a.seen.lock().unwrap()[0]
            .reasoning_effort
            .as_ref()
            .unwrap()
            .as_str(),
        "standard"
    );
    k.shutdown().await;
}

struct DelayedAdapter {
    started: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    release: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
    seen: AtomicUsize,
}
impl LlmAdapter for DelayedAdapter {
    fn resolve_model<'a>(
        &'a self,
        _p: &'a str,
        _m: &'a str,
        _s: Option<&'a AbortSignal>,
    ) -> LlmFuture<'a, LlmResolvedModelInfo> {
        let started = self.started.lock().unwrap().take();
        let release = self.release.lock().unwrap().take().unwrap();
        Box::pin(async move {
            if let Some(s) = started {
                s.send(()).unwrap();
            }
            release.await.unwrap();
            Ok(reasoning_info(true))
        })
    }
    fn stream(&self, _: GenerateOptions) -> Result<LlmStream, BoxError> {
        self.seen.fetch_add(1, Ordering::SeqCst);
        Ok(LlmStream::chunks(script()))
    }
}
#[tokio::test]
async fn pins_one_adapter_registration_across_asynchronous_exact_model_resolution_and_dispatch() {
    let k = Kernel::start();
    let c = k.root();
    let r = LlmRuntime::new();
    let (st, started) = tokio::sync::oneshot::channel();
    let (release, rx) = tokio::sync::oneshot::channel();
    let first = Arc::new(DelayedAdapter {
        started: Mutex::new(Some(st)),
        release: Mutex::new(Some(rx)),
        seen: AtomicUsize::new(0),
    });
    let h = r
        .register_adapter(&c, vec!["route".into()], first.clone())
        .unwrap();
    let rr = r.clone();
    let cc = c.clone();
    let drain = tokio::spawn(async move { collect(rr.stream(&cc, options())).await.unwrap() });
    started.await.unwrap();
    h.dispose();
    let second = Arc::new(RecordingAdapter::default());
    r.register_adapter(&c, vec!["route".into()], second.clone())
        .unwrap();
    release.send(()).unwrap();
    drain.await.unwrap();
    assert_eq!(first.seen.load(Ordering::SeqCst), 1);
    assert!(second.seen.lock().unwrap().is_empty());
    k.shutdown().await;
}
#[tokio::test]
async fn prepares_a_one_shot_registration_bound_call_and_rejects_config_drift() {
    let (k, c, r, a) = setup().await;
    *a.metadata.lock().unwrap() = Some(reasoning_info(true));
    let p = r.prepare_call(&c, config(), None, None).await.unwrap();
    assert_eq!(p.adapter_defaults.reasoning_effort, Some(true));
    let mut wrong = options().with_call_config(&p.config);
    wrong.model = "other".into();
    assert_eq!(
        p.stream(wrong).err().unwrap().code(),
        "INVALID_PREPARED_CALL"
    );
    collect(p.stream(options().with_call_config(&p.config)).unwrap())
        .await
        .unwrap();
    assert_eq!(
        p.stream(options().with_call_config(&p.config))
            .err()
            .unwrap()
            .code(),
        "INVALID_PREPARED_CALL"
    );
    let p = r.prepare_call(&c, config(), None, None).await.unwrap();
    let listener = c
        .on_waterfall::<LlmStreamEvent, _, _>(|e, next| async move {
            e.update_options(|o| o.model = "other".into())?;
            next.run().await
        })
        .unwrap();
    let out = collect(p.stream(options().with_call_config(&p.config)).unwrap())
        .await
        .unwrap();
    assert_eq!(
        value(out)[0]["reason"]["failure"]["code"],
        "INVALID_PREPARED_CALL"
    );
    drop(listener.dispose());
    k.shutdown().await;
}
#[tokio::test]
async fn preserves_concrete_reasoning_selection_during_preparation() {
    let (k, c, r, a) = setup().await;
    *a.metadata.lock().unwrap() = Some(reasoning_info(true));
    for selection in [None, Some("ultra")] {
        let mut conf = config();
        conf.controls.reasoning_effort = selection.map(Into::into);
        let p = r.prepare_call(&c, conf, None, None).await.unwrap();
        assert_eq!(
            p.config
                .controls
                .reasoning_effort
                .as_ref()
                .unwrap()
                .as_str(),
            selection.unwrap_or("standard")
        );
        assert_eq!(
            p.adapter_defaults.reasoning_effort,
            selection.is_none().then_some(true)
        );
        collect(p.stream(options().with_call_config(&p.config)).unwrap())
            .await
            .unwrap();
        assert_eq!(
            a.seen.lock().unwrap().last().unwrap().reasoning_effort,
            p.config.controls.reasoning_effort
        );
    }
    k.shutdown().await;
}
#[tokio::test]
async fn composes_controls_once_before_defaults_while_retaining_the_captured_route() {
    let (k, c, r, a) = setup().await;
    let mut info = reasoning_info(true);
    info.default_max_tokens = Some(8192);
    *a.metadata.lock().unwrap() = Some(info);
    let calls = AtomicUsize::new(0);
    let configure = |controls: &LlmCallControls, model: &LlmResolvedModelInfo| {
        calls.fetch_add(1, Ordering::SeqCst);
        assert_eq!(controls.temperature, Some(0.2));
        assert_eq!(controls.stop.as_ref().unwrap(), &["END"]);
        let mut controls = controls.clone();
        controls.reasoning_effort = Some(model.reasoning.as_ref().unwrap().efforts[0].id.clone());
        controls.max_tokens = Some(256);
        controls.max_tokens = Some(128);
        Ok(controls)
    };
    let conf = LlmCallConfig {
        controls: LlmCallControls {
            temperature: Some(0.2),
            stop: Some(vec!["END".into()]),
            ..Default::default()
        },
        ..config()
    };
    let p = r
        .prepare_call(&c, conf, None, Some(&configure))
        .await
        .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(p.config.provider, "route");
    assert_eq!(p.config.controls.max_tokens, Some(128));
    assert_eq!(p.adapter_defaults, LlmCallConfigAdapterDefaults::default());
    collect(p.stream(options().with_call_config(&p.config)).unwrap())
        .await
        .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    k.shutdown().await;
}
#[tokio::test]
async fn applies_defaults_to_controls_omitted_by_the_configuration_function() {
    let (k, c, r, a) = setup().await;
    let mut info = reasoning_info(true);
    info.default_max_tokens = Some(8192);
    *a.metadata.lock().unwrap() = Some(info);
    let mut proposed = config();
    proposed.controls.reasoning_effort = Some("unsupported".into());
    proposed.controls.max_tokens = Some(64);
    let configure = |_: &LlmCallControls, _: &LlmResolvedModelInfo| Ok(LlmCallControls::default());
    let p = r
        .prepare_call(&c, proposed, None, Some(&configure))
        .await
        .unwrap();
    assert_eq!(p.config.controls.max_tokens, Some(8192));
    assert_eq!(
        p.config
            .controls
            .reasoning_effort
            .as_ref()
            .unwrap()
            .as_str(),
        "standard"
    );
    assert_eq!(
        p.adapter_defaults,
        LlmCallConfigAdapterDefaults {
            max_tokens: Some(true),
            reasoning_effort: Some(true)
        }
    );
    let configure = |_: &LlmCallControls, _: &LlmResolvedModelInfo| {
        Ok(LlmCallControls {
            max_tokens: Some(8192),
            reasoning_effort: Some("standard".into()),
            ..Default::default()
        })
    };
    assert_eq!(
        r.prepare_call(&c, config(), None, Some(&configure))
            .await
            .unwrap()
            .adapter_defaults,
        LlmCallConfigAdapterDefaults::default()
    );
    k.shutdown().await;
}
#[tokio::test]
async fn rejects_callback_failures_and_unsupported_configured_effort_before_dispatch() {
    let (k, c, r, a) = setup().await;
    let configure = |_: &LlmCallControls, _: &LlmResolvedModelInfo| {
        Err(ValueError::Message("configuration unavailable".into()).into())
    };
    assert_eq!(
        r.prepare_call(&c, config(), None, Some(&configure))
            .await
            .err()
            .unwrap()
            .to_string(),
        "configuration unavailable"
    );
    let configure = |_: &LlmCallControls, _: &LlmResolvedModelInfo| {
        Ok(LlmCallControls {
            reasoning_effort: Some("unsupported".into()),
            ..Default::default()
        })
    };
    assert_eq!(
        code(
            r.prepare_call(&c, config(), None, Some(&configure))
                .await
                .err()
                .unwrap()
                .as_ref()
        ),
        "UNSUPPORTED_REASONING_EFFORT"
    );
    assert!(a.seen.lock().unwrap().is_empty());
    k.shutdown().await;
}
struct CancellingAdapter {
    abort: Arc<AbortController>,
    lookups: AtomicUsize,
}
impl LlmAdapter for CancellingAdapter {
    fn resolve_model<'a>(
        &'a self,
        _p: &'a str,
        _m: &'a str,
        _s: Option<&'a AbortSignal>,
    ) -> LlmFuture<'a, LlmResolvedModelInfo> {
        self.lookups.fetch_add(1, Ordering::SeqCst);
        self.abort
            .abort_with(ValueError::Message("cancelled preparation".into()));
        Box::pin(async { Ok(metadata()) })
    }
    fn stream(&self, _: GenerateOptions) -> Result<LlmStream, BoxError> {
        panic!("cancelled call dispatched")
    }
}
#[tokio::test]
async fn skips_configuration_when_cancellation_precedes_preparation_or_occurs_during_model_lookup()
{
    let k = Kernel::start();
    let c = k.root();
    let r = LlmRuntime::new();
    let abort = Arc::new(AbortController::new());
    let adapter = Arc::new(CancellingAdapter {
        abort: abort.clone(),
        lookups: AtomicUsize::new(0),
    });
    r.register_adapter(&c, vec!["route".into()], adapter.clone())
        .unwrap();
    let configurations = AtomicUsize::new(0);
    let configure = |controls: &LlmCallControls, _: &LlmResolvedModelInfo| {
        configurations.fetch_add(1, Ordering::SeqCst);
        Ok(controls.clone())
    };
    for _ in 0..2 {
        let e = r
            .prepare_call(&c, config(), Some(abort.signal()), Some(&configure))
            .await
            .err()
            .unwrap();
        assert_eq!(e.to_string(), "cancelled preparation");
        assert!(
            SharedError::of(e.as_ref())
                .unwrap()
                .ptr_eq(&abort.signal().reason().unwrap())
        );
    }
    assert_eq!(adapter.lookups.load(Ordering::SeqCst), 1);
    assert_eq!(configurations.load(Ordering::SeqCst), 0);
    k.shutdown().await;
}
#[tokio::test]
async fn rejects_cancellation_raised_during_configuration_before_returning_a_prepared_call() {
    let (k, c, r, a) = setup().await;
    let abort = AbortController::new();
    let configure = |controls: &LlmCallControls, _: &LlmResolvedModelInfo| {
        abort.abort_with(ValueError::Message("cancelled configuration".into()));
        Ok(controls.clone())
    };
    assert_eq!(
        r.prepare_call(&c, config(), Some(abort.signal()), Some(&configure))
            .await
            .err()
            .unwrap()
            .to_string(),
        "cancelled configuration"
    );
    assert!(a.seen.lock().unwrap().is_empty());
    k.shutdown().await;
}
struct CountingAdapter {
    lookups: Arc<AtomicUsize>,
}
impl LlmAdapter for CountingAdapter {
    fn resolve_model<'a>(
        &'a self,
        _p: &'a str,
        _m: &'a str,
        _s: Option<&'a AbortSignal>,
    ) -> LlmFuture<'a, LlmResolvedModelInfo> {
        self.lookups.fetch_add(1, Ordering::SeqCst);
        let mut i = reasoning_info(true);
        i.context = Some(LlmModelContext {
            context_window: 128000,
        });
        Box::pin(async move { Ok(i) })
    }
    fn stream(&self, _: GenerateOptions) -> Result<LlmStream, BoxError> {
        Ok(LlmStream::chunks(script()))
    }
}
#[tokio::test]
async fn reuses_one_exact_model_lookup_for_prepared_config_and_context_metadata() {
    let k = Kernel::start();
    let c = k.root();
    let r = LlmRuntime::new();
    let lookups = Arc::new(AtomicUsize::new(0));
    r.register_adapter(
        &c,
        vec!["route".into()],
        Arc::new(CountingAdapter {
            lookups: lookups.clone(),
        }),
    )
    .unwrap();
    let p = r.prepare_call(&c, config(), None, None).await.unwrap();
    assert_eq!(p.context.as_ref().unwrap().context_window, 128000);
    collect(p.stream(options().with_call_config(&p.config)).unwrap())
        .await
        .unwrap();
    assert_eq!(lookups.load(Ordering::SeqCst), 1);
    r.prepare_call(&c, config(), None, None).await.unwrap();
    assert_eq!(lookups.load(Ordering::SeqCst), 2);
    k.shutdown().await;
}
struct GenerationAdapter {
    generation: Arc<Mutex<String>>,
    dispatched: Arc<Mutex<Option<String>>>,
}
impl LlmAdapter for GenerationAdapter {
    fn stream(&self, _: GenerateOptions) -> Result<LlmStream, BoxError> {
        panic!("unbound dispatch")
    }
    fn prepare_call(
        self: Arc<Self>,
        _provider: String,
        _model: String,
        _signal: Option<AbortSignal>,
    ) -> LlmFuture<'static, PreparedAdapterCall> {
        let captured = self.generation.lock().unwrap().clone();
        let mut model = metadata();
        model.model.input_modalities = Some(vec!["text".into()]);
        model.reasoning = Some(LlmModelReasoningInfo {
            efforts: vec![LlmReasoningEffortInfo {
                id: captured.clone().into(),
                name: captured.clone(),
                description: None,
            }],
            default_effort: None,
        });
        Box::pin(async move {
            Ok(PreparedAdapterCall {
                model,
                dispatch: Arc::new(move |_| {
                    *self.dispatched.lock().unwrap() = Some(captured.clone());
                    Ok(LlmStream::chunks(script()))
                }),
            })
        })
    }
}
#[tokio::test]
async fn binds_adapter_owned_capabilities_and_dispatch_to_one_prepared_generation() {
    let k = Kernel::start();
    let c = k.root();
    let r = LlmRuntime::new();
    let generation = Arc::new(Mutex::new("first".into()));
    let dispatched = Arc::new(Mutex::new(None));
    r.register_adapter(
        &c,
        vec!["route".into()],
        Arc::new(GenerationAdapter {
            generation: generation.clone(),
            dispatched: dispatched.clone(),
        }),
    )
    .unwrap();
    let configure = |controls: &LlmCallControls, model: &LlmResolvedModelInfo| {
        Ok(LlmCallControls {
            reasoning_effort: Some(model.reasoning.as_ref().unwrap().efforts[0].id.clone()),
            ..controls.clone()
        })
    };
    let p = r
        .prepare_call(&c, config(), None, Some(&configure))
        .await
        .unwrap();
    *generation.lock().unwrap() = "second".into();
    assert_eq!(
        p.config
            .controls
            .reasoning_effort
            .as_ref()
            .unwrap()
            .as_str(),
        "first"
    );
    assert_eq!(p.input_modalities.as_ref().unwrap(), &["text"]);
    collect(p.stream(options().with_call_config(&p.config)).unwrap())
        .await
        .unwrap();
    assert_eq!(dispatched.lock().unwrap().as_deref(), Some("first"));
    k.shutdown().await;
}
#[tokio::test]
async fn projects_historical_images_to_stable_text_only_after_the_loop_visible_waterfall() {
    let (k, c, r, a) = setup().await;
    let mut info = metadata();
    info.model.input_modalities = Some(vec!["text".into()]);
    *a.metadata.lock().unwrap() = Some(info);
    let observed = Arc::new(Mutex::new(vec![]));
    let s = observed.clone();
    c.on_waterfall::<LlmStreamEvent, _, _>(move |e, next| {
        let s = s.clone();
        async move {
            s.lock().unwrap().push(e.options());
            next.run().await
        }
    })
    .unwrap();
    let mut o = options();
    o.messages = vec![user(vec![image(3, false)])];
    mark_agent_loop_request(&mut o);
    collect(r.stream(&c, o)).await.unwrap();
    assert_eq!(
        observed.lock().unwrap()[0].messages[0].content()[0].r#type,
        "image"
    );
    assert_eq!(
        a.seen.lock().unwrap()[0].messages[0].content()[0]
            .text_value()
            .unwrap(),
        "[image omitted because this model accepts text only; attachment sha256:aaaaaaaa]"
    );
    assert!(!is_agent_loop_request(&a.seen.lock().unwrap()[0]));
    k.shutdown().await;
}
struct WaitingAdapter;
impl LlmAdapter for WaitingAdapter {
    fn stream(&self, _: GenerateOptions) -> Result<LlmStream, BoxError> {
        panic!("unexpected dispatch")
    }
    fn resolve_model<'a>(
        &'a self,
        _p: &'a str,
        _m: &'a str,
        signal: Option<&'a AbortSignal>,
    ) -> LlmFuture<'a, LlmResolvedModelInfo> {
        Box::pin(async move {
            let signal = signal.unwrap();
            signal.cancelled().await;
            signal.throw_if_aborted()?;
            Ok(metadata())
        })
    }
}
#[tokio::test]
async fn passes_cancellation_through_exact_model_resolution() {
    let k = Kernel::start();
    let c = k.root();
    let r = LlmRuntime::new();
    r.register_adapter(&c, vec!["route".into()], Arc::new(WaitingAdapter))
        .unwrap();
    let abort = AbortController::new();
    let signal = abort.signal();
    let resolving = r.resolve_call_config(config(), Some(&signal));
    let aborting = async {
        tokio::task::yield_now().await;
        abort.abort_with(ValueError::Message("cancel reasoning".into()));
    };
    let (result, ()) = tokio::join!(resolving, aborting);
    assert_eq!(result.unwrap_err().to_string(), "cancel reasoning");
    k.shutdown().await;
}
#[test]
fn rejects_invalid_adapter_model_context() {
    let mut info = metadata();
    info.context = Some(LlmModelContext { context_window: 0 });
    assert_eq!(
        normalize_model_info("route", "model", info)
            .unwrap_err()
            .code(),
        "INVALID_MODEL_CONTEXT"
    );
    for n in [json!(-1), json!(1.5), json!(null)] {
        assert!(serde_json::from_value::<LlmModelContext>(json!({"contextWindow":n})).is_err());
    }
}
#[tokio::test]
async fn captures_a_declared_in_history_system_prompt_update_mode_and_rejects_any_other_mode() {
    let (k, c, r, a) = setup().await;
    let mut info = metadata();
    info.system_prompt_update = Some("in-history".into());
    *a.metadata.lock().unwrap() = Some(info.clone());
    assert_eq!(
        r.resolve_model_info("route", "model", None)
            .await
            .unwrap()
            .system_prompt_update
            .as_deref(),
        Some("in-history")
    );
    assert_eq!(
        r.prepare_call(&c, config(), None, None)
            .await
            .unwrap()
            .system_prompt_update
            .as_deref(),
        Some("in-history")
    );
    info.system_prompt_update = Some("leading".into());
    assert_eq!(
        normalize_model_info("route", "model", info)
            .unwrap_err()
            .code(),
        "INVALID_MODEL_INFO"
    );
    *a.metadata.lock().unwrap() = Some(metadata());
    assert!(
        r.prepare_call(&c, config(), None, None)
            .await
            .unwrap()
            .system_prompt_update
            .is_none()
    );
    k.shutdown().await;
}
struct BadProvider;
impl LlmAdapter for BadProvider {
    fn provider_info(&self, _: &str) -> Result<LlmProviderInfo, BoxError> {
        Ok(LlmProviderInfo {
            id: "other".into(),
            name: "Name".into(),
        })
    }
    fn stream(&self, _: GenerateOptions) -> Result<LlmStream, BoxError> {
        panic!("invalid provider dispatched")
    }
}
#[tokio::test]
async fn rejects_invalid_provider_metadata_atomically() {
    let k = Kernel::start();
    let c = k.root();
    let r = LlmRuntime::new();
    let e = r
        .register_adapter(&c, vec!["route".into()], Arc::new(BadProvider))
        .err()
        .unwrap();
    assert_eq!(code(e.as_ref()), "INVALID_ADAPTER");
    assert!(r.list_providers().is_empty());
    for v in [
        json!({"id":1,"name":"Name"}),
        json!({"id":"route","name":1}),
    ] {
        assert!(serde_json::from_value::<LlmProviderInfo>(v).is_err());
    }
    k.shutdown().await;
}
#[tokio::test]
async fn rejects_invalid_model_metadata() {
    let (k, _, r, a) = setup().await;
    for model in [
        json!({"provider":"other","id":"m","name":"M"}),
        json!({"provider":"route","id":"","name":"M"}),
        json!({"provider":"route","id":"m","name":""}),
    ] {
        *a.models.lock().unwrap() = vec![decode(model)];
        assert_eq!(
            code(r.list_models("route").await.unwrap_err().as_ref()),
            "INVALID_CATALOG"
        );
    }
    for v in [
        json!({"provider":1,"id":"m","name":"M"}),
        json!({"provider":"route","id":1,"name":"M"}),
        json!({"provider":"route","id":"m","name":1}),
        json!({"provider":"route","id":"m","name":"M","description":1}),
    ] {
        assert!(serde_json::from_value::<LlmModelInfo>(v).is_err());
    }
    k.shutdown().await;
}
#[tokio::test]
async fn rejects_duplicate_model_ids_in_one_provider_catalog() {
    let (k, _, r, a) = setup().await;
    let m = metadata().model;
    *a.models.lock().unwrap() = vec![m.clone(), m];
    assert_eq!(
        code(r.list_models("route").await.unwrap_err().as_ref()),
        "INVALID_CATALOG"
    );
    k.shutdown().await;
}
#[tokio::test]
async fn lets_llm_stream_waterfall_listeners_wrap_the_underlying_stream() {
    let (k, c, r, _) = setup().await;
    c.on_waterfall::<LlmStreamEvent, _, _>(|_, next| async move {
        let inner = next.run().await?;
        let prefix = vec![
            Ok(chunk(
                json!({"type":"block-start","index":99,"blockType":"text"}),
            )),
            Ok(chunk(
                json!({"type":"block-end","index":99,"block":{"type":"text","text":""}}),
            )),
        ];
        Ok(LlmStream::new(
            futures_util::stream::iter(prefix).chain(inner),
        ))
    })
    .unwrap();
    let out = collect(r.stream(&c, options())).await.unwrap();
    assert_eq!(out.len(), 6);
    assert_eq!(value(&out)[0]["index"], 99);
    k.shutdown().await;
}
#[tokio::test]
async fn resolves_the_provider_after_llm_stream_listeners_have_had_a_chance_to_route_it() {
    let (k, c, r, a) = setup().await;
    c.on_waterfall::<LlmStreamEvent, _, _>(|e, next| async move {
        e.update_options(|o| o.provider = "route".into())?;
        next.run().await
    })
    .unwrap();
    let mut o = options();
    o.provider = "initial".into();
    collect(r.stream(&c, o)).await.unwrap();
    assert_eq!(a.seen.lock().unwrap()[0].provider, "route");
    k.shutdown().await;
}
fn historical() -> RequestMessage {
    create_assistant_message(NewAssistantMessage {
        content: vec![ContentBlock::text("old response")],
        source: AssistantProviderMetadata {
            provider: "historical".into(),
            model: "old-model".into(),
            replay_state: Some(json!({"private":"state"})),
        },
    })
    .into()
}
#[tokio::test]
async fn dispatches_identity_free_user_input_beside_durable_assistant_history() {
    let (k, c, r, a) = setup().await;
    let input: RequestMessage =
        decode(json!({"role":"user","content":[{"type":"text","text":"summarize"}]}));
    let mut o = options();
    o.messages = vec![historical(), input.clone()];
    collect(r.stream(&c, o)).await.unwrap();
    let seen = a.seen.lock().unwrap().clone();
    assert_eq!(seen[0].messages[1], input);
    assert!(seen[0].messages[1].id().is_none());
    assert!(
        value(&seen[0].messages[0])["source"]
            .get("replayState")
            .is_none()
    );
    drop(seen);
    k.shutdown().await;
}
#[tokio::test]
async fn keeps_replay_state_when_historical_and_target_providers_belong_to_the_same_adapter_instance()
 {
    let (k, c, r, a) = setup().await;
    r.register_adapter(&c, vec!["historical".into()], a.clone())
        .unwrap();
    let mut o = options();
    o.messages = vec![historical()];
    collect(r.stream(&c, o)).await.unwrap();
    assert_eq!(
        value(&a.seen.lock().unwrap()[0].messages[0])["source"]["replayState"],
        json!({"private":"state"})
    );
    k.shutdown().await;
}
#[tokio::test]
async fn strips_replay_state_but_preserves_provider_and_model_when_the_target_uses_a_different_adapter_instance()
 {
    let (k, c, r, a) = setup().await;
    r.register_adapter(
        &c,
        vec!["historical".into()],
        Arc::new(RecordingAdapter::default()),
    )
    .unwrap();
    let mut o = options();
    o.messages = vec![historical()];
    collect(r.stream(&c, o)).await.unwrap();
    assert_eq!(
        value(&a.seen.lock().unwrap()[0].messages[0])["source"],
        json!({"kind":"model","provider":"historical","model":"old-model"})
    );
    k.shutdown().await;
}
#[tokio::test]
async fn preserves_immutability_while_stripping_replay_state_from_frozen_requests() {
    let (k, c, r, a) = setup().await;
    let h = historical();
    let mut o = options();
    o.messages = vec![h.clone()];
    mark_agent_loop_request(&mut o);
    collect(r.stream(&c, o)).await.unwrap();
    assert!(
        value(&a.seen.lock().unwrap()[0].messages[0])["source"]
            .get("replayState")
            .is_none()
    );
    assert_eq!(
        value(h)["source"]["replayState"],
        json!({"private":"state"})
    );
    k.shutdown().await;
}
#[test]
fn creates_llm_error_with_a_code_for_programmatic_handling() {
    let e = LlmError::from_failure(
        &LlmFailure::new("something went wrong", "CUSTOM_CODE"),
        None,
    )
    .unwrap();
    assert_eq!(e.name(), "LlmError");
    assert_eq!(e.message(), "something went wrong");
    assert_eq!(e.code(), "CUSTOM_CODE");
}
#[test]
fn rejects_non_serializable_structured_failure_facts_at_construction() {
    assert_eq!(
        LlmError::new("", BuiltinLlmFailureCode::RateLimit)
            .unwrap_err()
            .to_string(),
        "LlmError message must be a non-empty string"
    );
    for (o, text) in [
        (
            LlmErrorOptions {
                status: Some(42),
                ..Default::default()
            },
            "status",
        ),
        (
            LlmErrorOptions {
                provider_retry_after_ms: Some(f64::NAN),
                ..Default::default()
            },
            "providerRetryAfterMs",
        ),
        (
            LlmErrorOptions {
                request_id: Some("".into()),
                ..Default::default()
            },
            "requestId",
        ),
    ] {
        assert!(
            LlmError::with_options("busy", BuiltinLlmFailureCode::RateLimit, o)
                .unwrap_err()
                .to_string()
                .contains(text)
        );
    }
}
#[test]
fn llm_error_extends_the_shared_harness_error_base() {
    let cause = SharedError::new(ValueError::Message("root cause".into()));
    let e = LlmError::with_options(
        "boom",
        BuiltinLlmFailureCode::Auth,
        LlmErrorOptions {
            cause: Some(cause.clone()),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(is_harness_error(&e));
    assert_eq!(e.code(), "AUTH");
    assert!(SharedError::of(e.cause().unwrap()).unwrap().ptr_eq(&cause));
}
#[test]
fn harness_error_carries_a_code_names_itself_by_subclass_and_chains_cause() {
    let e = HarnessError::new("wrapper", "UNKNOWN")
        .with_cause(ValueError::Message("root cause".into()));
    assert_eq!(e.name(), "HarnessError");
    assert_eq!(e.code(), "UNKNOWN");
    assert_eq!(e.cause().unwrap().to_string(), "root cause");
    assert!(is_harness_error(&e));
    assert!(!is_harness_error(&ValueError::Message("nope".into())));
}
#[tokio::test]
async fn removes_the_adapter_when_the_returned_disposer_is_called() {
    let k = Kernel::start();
    let c = k.root();
    let r = LlmRuntime::new();
    let h = r
        .register_adapter(&c, vec!["m1".into()], Arc::new(RecordingAdapter::default()))
        .unwrap();
    assert_eq!(r.list_providers().len(), 1);
    h.dispose();
    assert!(r.list_providers().is_empty());
    k.shutdown().await;
}
#[tokio::test]
async fn rejects_duplicate_adapter_registration_with_duplicate_adapter_code() {
    let (k, c, r, _) = setup().await;
    let e = r
        .register_adapter(
            &c,
            vec!["route".into()],
            Arc::new(RecordingAdapter::default()),
        )
        .err()
        .unwrap();
    assert_eq!(code(e.as_ref()), "DUPLICATE_ADAPTER");
    assert!(e.to_string().contains("already registered"));
    k.shutdown().await;
}
#[tokio::test]
async fn rejects_empty_and_internally_duplicated_provider_registrations_atomically() {
    let k = Kernel::start();
    let c = k.root();
    let r = LlmRuntime::new();
    for (providers, expected) in [
        (vec![], "INVALID_ADAPTER"),
        (vec!["".into()], "INVALID_ADAPTER"),
        (vec!["first".into(), "first".into()], "DUPLICATE_ADAPTER"),
    ] {
        let e = r
            .register_adapter(&c, providers, Arc::new(RecordingAdapter::default()))
            .err()
            .unwrap();
        assert_eq!(code(e.as_ref()), expected);
        assert!(r.list_providers().is_empty());
    }
    k.shutdown().await;
}
#[tokio::test]
async fn re_registers_a_model_after_its_prior_registration_is_disposed() {
    let k = Kernel::start();
    let c = k.root();
    let r = LlmRuntime::new();
    for _ in 0..2 {
        let h = r
            .register_adapter(&c, vec!["m1".into()], Arc::new(RecordingAdapter::default()))
            .unwrap();
        assert_eq!(r.list_providers().len(), 1);
        h.dispose();
        assert!(r.list_providers().is_empty());
    }
    k.shutdown().await;
}
#[tokio::test]
async fn refuses_to_replace_routes_on_a_registration_that_was_already_released() {
    let (k, c, r, a) = setup().await;
    let h = r.register_adapter(&c, vec!["m1".into()], a).unwrap();
    h.dispose();
    assert!(
        h.replace(vec!["leaked".into()])
            .unwrap_err()
            .to_string()
            .contains("disposed adapter registration cannot replace its routes")
    );
    assert_eq!(r.list_providers().len(), 1);
    k.shutdown().await;
}
#[tokio::test]
async fn still_allows_an_empty_route_set_on_a_live_registration() {
    let k = Kernel::start();
    let c = k.root();
    let r = LlmRuntime::new();
    let h = r
        .register_adapter(&c, vec!["m1".into()], Arc::new(RecordingAdapter::default()))
        .unwrap();
    h.replace(vec![]).unwrap();
    assert!(r.list_providers().is_empty());
    h.replace(vec!["m2".into()]).unwrap();
    assert_eq!(r.list_providers()[0].id, "m2");
    h.dispose();
    assert!(r.list_providers().is_empty());
    k.shutdown().await;
}
struct ResizingAdapter;
impl LlmAdapter for ResizingAdapter {
    fn stream(&self, _: GenerateOptions) -> Result<LlmStream, BoxError> {
        Ok(LlmStream::chunks([]))
    }
    fn image_request_target(
        &self,
        _: &str,
        _: &str,
        _: &hoh_attachment::ImageAttachmentRef,
    ) -> hoh_attachment::ImageRequestTarget {
        hoh_attachment::ImageRequestTarget {
            width: 320,
            height: 240,
            max_bytes: 100,
        }
    }
}
#[tokio::test]
async fn delegates_image_coordinate_projections_and_preserves_images_for_adapters_without_a_resize_policy()
 {
    let (k, c, r, _) = setup().await;
    let reference = decode(
        json!({"attachmentId":format!("sha256:{}","a".repeat(64)),"mediaType":"image/png","bytes":200,"width":640,"height":480}),
    );
    assert_eq!(
        value(
            r.image_request_target("route", "vision", &reference)
                .unwrap()
        ),
        json!({"width":640,"height":480,"maxBytes":200})
    );
    r.register_adapter(&c, vec!["resized".into()], Arc::new(ResizingAdapter))
        .unwrap();
    assert_eq!(
        value(
            r.image_request_target("resized", "vision", &reference)
                .unwrap()
        ),
        json!({"width":320,"height":240,"maxBytes":100})
    );
    assert!(
        r.image_request_target("absent", "vision", &reference)
            .is_err()
    );
    k.shutdown().await;
}
#[tokio::test]
async fn rejects_loop_request_mutation_and_snapshot_config_drift() {
    let (k, c, r, a) = setup().await;
    let listener = c
        .on_waterfall::<LlmStreamEvent, _, _>(|e, next| async move {
            assert!(is_agent_loop_request(&e.options()));
            assert!(e.update_options(|o| o.model = "other".into()).is_err());
            next.run().await
        })
        .unwrap();
    let mut o = options();
    mark_agent_loop_request(&mut o);
    collect(r.stream(&c, o)).await.unwrap();
    assert_eq!(a.seen.lock().unwrap()[0].model, "model");
    assert!(is_agent_loop_request(&a.seen.lock().unwrap()[0]));
    drop(listener.dispose());
    let mut p = r.prepare_call(&c, config(), None, None).await.unwrap();
    p.config.model = "other".into();
    assert_eq!(
        p.stream(options().with_call_config(&p.config))
            .err()
            .unwrap()
            .code(),
        "INVALID_PREPARED_CALL"
    );
    k.shutdown().await;
}

#[test]
fn respects_utf16_lengths_and_javascript_line_terminators_in_error_classification() {
    assert!(is_context_window_exceeded_error(&format!(
        "input{} exceeds the model context",
        "😀".repeat(19)
    )));
    assert!(!is_context_window_exceeded_error(&format!(
        "input{} exceeds the model context",
        "😀".repeat(20)
    )));
    for separator in ["\r", "\n", "\u{2028}", "\u{2029}"] {
        assert!(!is_context_window_exceeded_error(&format!(
            "input{separator}bad exceeds the model context"
        )));
    }
    assert!(!is_quota_exceeded_error("inſufficient_quota"));
}

#[derive(Debug)]
struct LinkedError(Option<Box<LinkedError>>);
impl std::fmt::Display for LinkedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("node")
    }
}
impl std::error::Error for LinkedError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.0.as_deref().map(|e| e as _)
    }
}
#[test]
fn renders_deep_error_chains_iteratively() {
    let mut chain = LinkedError(None);
    for _ in 0..10_000 {
        chain = LinkedError(Some(Box::new(chain)));
    }
    assert_eq!(error_chain(&chain), "node");
    while let Some(next) = chain.0.take() {
        chain = *next;
    }
}
