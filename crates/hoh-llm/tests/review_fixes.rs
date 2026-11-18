mod common;
use common::*;
use hoh_llm::*;
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

#[tokio::test]
async fn topology_ignores_registrant_receiver_filter() {
    let kernel = hoh_kernel::Kernel::start();
    let root = kernel.root();
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    root.on_contained::<AdaptersUpdated>(move |_| {
        seen.fetch_add(1, Ordering::SeqCst);
        Ok(())
    })
    .unwrap();
    let runtime_context = root.child("runtime").unwrap();
    let runtime = LlmRuntime::with_context(&runtime_context);
    root.on_contained::<AdaptersUpdated>(|_| {
        Err(ValueError::Message("observer failed".into()).into())
    })
    .unwrap();
    let handle = runtime
        .register_adapter(
            &root.with_filter(|_| false),
            vec!["route".into()],
            Arc::new(RecordingAdapter::default()),
        )
        .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    handle.dispose();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    let warnings = root
        .logger()
        .buffer()
        .into_iter()
        .filter(|r| r.level == hoh_kernel::Level::Warn)
        .collect::<Vec<_>>();
    assert_eq!(warnings.len(), 4);
    assert!(
        warnings
            .iter()
            .all(|r| r.fiber == runtime_context.fiber().uid())
    );
    kernel.shutdown().await;
}

#[test]
fn stream_json_extensions_keep_their_positions() {
    for raw in [
        r#"{"extension":1,"type":"usage","usage":{"cost":0.5,"outputTokens":2,"vendor":true,"inputTokens":1}}"#,
        r#"{"type":"finish","replayState":{"vendor":1,"response":{"raw":true},"blocks":[],"tail":2},"reason":{"failure":{"vendor":1,"code":"SERVER","message":"bad","tail":2},"kind":"error"}}"#,
        r#"{"index":0,"type":"block-end","block":{"text":"ok","vendor":true,"type":"text"}}"#,
    ] {
        let chunk: Value = serde_json::from_str(raw).unwrap();
        let mut acc = AssistantStreamAccumulator::new();
        let returned = acc.push_value(&json!({"time":1,"chunk":chunk})).unwrap();
        assert_eq!(serde_json::to_string(&returned.chunk).unwrap(), raw);
        let snapshot = serde_json::to_value(acc.snapshot()).unwrap();
        assert_eq!(serde_json::to_string(&snapshot[0]["chunk"]).unwrap(), raw);
        assert_eq!(
            serde_json::to_string(&expand_assistant_stream(&acc.snapshot()).unwrap()[0].chunk)
                .unwrap(),
            raw
        );
        let typed: StreamChunk = serde_json::from_str(raw).unwrap();
        let mut assembler = BlockAssembler::new();
        assembler.push(typed);
        if let Some(usage) = chunk.get("usage") {
            assert_eq!(
                serde_json::to_string(assembler.usage().unwrap()).unwrap(),
                serde_json::to_string(usage).unwrap()
            );
            assert_eq!(
                serde_json::to_string(
                    &last_assistant_stream_chunk(&acc.snapshot(), RawStreamChunkType::Usage)
                        .unwrap()
                )
                .unwrap(),
                raw
            );
        }
        if let Some(reason) = chunk.get("reason") {
            assert_eq!(
                serde_json::to_string(&assembler.finish()).unwrap(),
                serde_json::to_string(reason).unwrap()
            );
            assert_eq!(
                serde_json::to_string(&assembler.replay_state().unwrap().unwrap()).unwrap(),
                serde_json::to_string(&chunk["replayState"]).unwrap()
            );
        }
        if let Some(block) = chunk.get("block") {
            assert_eq!(
                serde_json::to_string(&assembler.blocks().unwrap()[0]).unwrap(),
                serde_json::to_string(block).unwrap()
            );
        }
    }
    let raw = r#"{"cost":0.5,"outputTokens":2,"inputTokens":1}"#;
    assert_eq!(
        serde_json::to_string(&serde_json::from_str::<TokenUsage>(raw).unwrap()).unwrap(),
        raw
    );
    let raw = r#"{"vendor":true,"code":"SERVER","message":"bad"}"#;
    assert_eq!(
        serde_json::to_string(&serde_json::from_str::<LlmFailure>(raw).unwrap()).unwrap(),
        raw
    );
    let mut usage: TokenUsage =
        serde_json::from_str(r#"{"cost":0.5,"outputTokens":2,"inputTokens":1,"totalTokens":null}"#)
            .unwrap();
    usage.input_tokens = 3;
    assert_eq!(
        serde_json::to_string(&usage).unwrap(),
        r#"{"cost":0.5,"outputTokens":2,"inputTokens":3,"totalTokens":null}"#
    );
}

#[test]
fn projection_uses_block_tag_even_when_attachment_is_not_typed() {
    let messages = vec![user(vec![block(
        json!({"type":"image","attachment":{"attachmentId":"sha256:abcdefgh12345678","mediaType":"image/tiff","bytes":1,"width":1,"height":1}}),
    )])];
    assert_eq!(
        project_images_for_text_model(&messages)[0].content()[0],
        ContentBlock::text(
            "[image omitted because this model accepts text only; attachment sha256:abcdefgh]"
        )
    );
    let messages = vec![user(vec![block(
        json!({"type":"file","attachment":{"attachmentId":"sha256:abcdefgh12345678","name":"x","bytes":1.5}}),
    )])];
    let projected = project_files_to_text(&messages, |_| None);
    assert_eq!(projected[0].content()[0].r#type, "text");
    assert!(
        projected[0].content()[0]
            .text_value()
            .unwrap()
            .contains("1.5 bytes")
    );
}

#[test]
fn serialization_matches_source_property_order() {
    let tool: ToolSchema =
        decode(json!({"name":"x","description":"d","parameters":{},"deferLoading":true}));
    assert_eq!(
        serde_json::to_string(&tool).unwrap(),
        r#"{"name":"x","description":"d","parameters":{},"deferLoading":true}"#
    );
    let info: LlmResolvedModelInfo = decode(
        json!({"provider":"p","id":"m","name":"M","systemPromptUpdate":"append","toolUpdate":"in-history","reasoning":{"efforts":[]}}),
    );
    assert_eq!(
        serde_json::to_string(&info).unwrap(),
        r#"{"provider":"p","id":"m","name":"M","systemPromptUpdate":"append","toolUpdate":"in-history","reasoning":{"efforts":[]}}"#
    );
}

struct CleanupAdapter(Arc<AtomicUsize>);
impl LlmAdapter for CleanupAdapter {
    fn stream(&self, _: GenerateOptions) -> Result<LlmStream, hoh_error::BoxError> {
        let count = self.0.clone();
        Ok(LlmStream::chunks(script()).with_cleanup(move || {
            Box::pin(async move {
                count.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
        }))
    }
}
#[tokio::test]
async fn dropping_unfinished_adapter_stream_runs_fiber_owned_cleanup() {
    use futures_util::StreamExt;
    let kernel = hoh_kernel::Kernel::start();
    let ctx = kernel.root();
    let runtime = LlmRuntime::new();
    let count = Arc::new(AtomicUsize::new(0));
    runtime
        .register_adapter(
            &ctx,
            vec!["route".into()],
            Arc::new(CleanupAdapter(count.clone())),
        )
        .unwrap();
    let mut stream = runtime.stream(&ctx, options());
    stream.next().await.unwrap().unwrap();
    drop(stream);
    tokio::task::yield_now().await;
    assert_eq!(count.load(Ordering::SeqCst), 1);
    kernel.shutdown().await;
}

#[test]
fn retry_schema_returns_nested_defaulted_config_independently_of_resolver() {
    let got = value(RetryPolicySchema::validate(&json!({"mode":"normal"})).unwrap());
    assert_eq!(
        got,
        json!({"mode":"normal","maxRetries":5,"retryableCodes":["EMPTY_RESPONSE","RATE_LIMIT","SERVER","TIMEOUT","TRANSPORT"],"backoff":{"initialDelayMs":500,"maxDelayMs":10000,"jitterRatio":0.1}})
    );
    assert!(RetryPolicySchema::validate(&json!({"mode":"normal","retryableCodes":["X","X"],"extension":true,"backoff":{"initialDelayMs":20,"maxDelayMs":10}})).is_ok());
    assert_eq!(
        value(RetryPolicySchema::validate(&Value::Null).unwrap()),
        Value::Null
    );
    let err = RetryPolicySchema::validate(&json!({"mode":"other"}))
        .unwrap_err()
        .to_string();
    assert!(
        err.starts_with("expected { mode: \"normal\", maxRetries?: number, retryableCodes?: ("),
        "{err}"
    );
    assert!(err.ends_with("but got {\"mode\":\"other\"}"), "{err}");
}

#[test]
fn rebuilt_failure_uses_only_source_selected_fields() {
    let failure: LlmFailure = decode(json!({"vendor":true,"code":"SERVER","message":"bad"}));
    let error = hoh_llm::adapter_failure::AdapterFailure {
        message: "bad".into(),
        code: "SERVER".into(),
        failure,
    };
    assert_eq!(
        value(hoh_llm::adapter_failure::normalize_llm_failure(&error)),
        json!({"message":"bad","code":"SERVER"})
    );
    let raised = LlmError::from_failure(&error.failure, None).unwrap();
    assert_eq!(
        serde_json::to_string(raised.detail::<LlmFailure>().unwrap()).unwrap(),
        r#"{"message":"bad","code":"SERVER"}"#
    );
}
#[test]
fn pruned_replay_uses_only_source_selected_fields() {
    let mut assembler = BlockAssembler::new();
    assembler
        .push_value(json!({"type":"block-end","index":0,"block":{"type":"text","text":"ok"}}))
        .unwrap();
    assembler.push_value(json!({"type":"block-end","index":1,"block":{"type":"tool-call","id":"x","name":"x","arguments":"{}"}})).unwrap();
    assembler.push_value(json!({"type":"finish","reason":{"kind":"max-tokens"},"replayState":{"vendor":true,"response":{"id":"r"},"blocks":["a","b"]}})).unwrap();
    assert_eq!(
        value(assembler.replay_state().unwrap()),
        json!({"response":{"id":"r"},"blocks":["a"]})
    );
}

#[test]
fn schema_preserves_nullable_union_members_and_reports_union_text() {
    assert_eq!(value(RetryPolicySchema::validate(&json!({"mode":"always","maxRetries":-1,"retryableCodes":[],"backoff":{"vendor":true}})).unwrap()),
        json!({"mode":"always","backoff":{"initialDelayMs":500,"maxDelayMs":10000,"jitterRatio":0.1,"vendor":true},"maxRetries":-1,"retryableCodes":[]}));
    assert_eq!(
        value(
            RetryPolicySchema::validate(
                &json!({"mode":"normal","retryableCodes":[null,"CUSTOM"],"backoff":null,"extra":1})
            )
            .unwrap()
        ),
        json!({"mode":"normal","maxRetries":5,"retryableCodes":[null,"CUSTOM"],"backoff":{"initialDelayMs":500,"maxDelayMs":10000,"jitterRatio":0.1},"extra":1})
    );
    // vendor/schemastery's object/union formatters, transcribed as a fixed oracle.
    let expected_type = r#"{ mode: "normal", maxRetries?: number, retryableCodes?: ("EMPTY_RESPONSE" | "MALFORMED_RESPONSE" | "QUOTA" | "RATE_LIMIT" | "SERVER" | "STREAM_CLOSED" | "TIMEOUT" | "TRANSPORT" | string)[], backoff?: { initialDelayMs?: number, maxDelayMs?: number, jitterRatio?: number } } | { mode: "always", backoff?: { initialDelayMs?: number, maxDelayMs?: number, jitterRatio?: number } }"#;
    for input in [
        json!({}),
        json!({"mode":"normal","maxRetries":1.5}),
        json!({"mode":"normal","retryableCodes":[""]}),
        json!({"mode":"always","backoff":{"initialDelayMs":0}}),
    ] {
        assert_eq!(
            RetryPolicySchema::validate(&input).unwrap_err().to_string(),
            format!(
                "expected {expected_type} but got {}",
                hoh_util_values::js_json_stringify(&input)
            )
        );
    }
}

struct DropProbe(Arc<AtomicUsize>);
impl Drop for DropProbe {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}
struct PendingCleanup {
    started: Arc<AtomicUsize>,
    dropped: Arc<AtomicUsize>,
}
impl LlmAdapter for PendingCleanup {
    fn stream(&self, _: GenerateOptions) -> Result<LlmStream, hoh_error::BoxError> {
        let started = self.started.clone();
        let dropped = self.dropped.clone();
        Ok(LlmStream::chunks(script()).with_cleanup(move || {
            Box::pin(async move {
                let _probe = DropProbe(dropped);
                started.fetch_add(1, Ordering::SeqCst);
                std::future::pending().await
            })
        }))
    }
}
#[tokio::test]
async fn dropped_stream_cleanup_is_cancelled_with_the_callers_fiber() {
    use futures_util::StreamExt;
    let kernel = hoh_kernel::Kernel::start();
    let root = kernel.root();
    let caller = root.child("caller").unwrap();
    let runtime = LlmRuntime::new();
    let started = Arc::new(AtomicUsize::new(0));
    let dropped = Arc::new(AtomicUsize::new(0));
    runtime
        .register_adapter(
            &root,
            vec!["route".into()],
            Arc::new(PendingCleanup {
                started: started.clone(),
                dropped: dropped.clone(),
            }),
        )
        .unwrap();
    let mut stream = runtime.stream(&caller, options());
    stream.next().await.unwrap().unwrap();
    drop(stream);
    tokio::task::yield_now().await;
    assert_eq!(started.load(Ordering::SeqCst), 1);
    assert_eq!(dropped.load(Ordering::SeqCst), 0);
    caller.fiber().dispose().await;
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
    kernel.shutdown().await;
}
#[tokio::test]
async fn explicit_close_and_exhaustion_do_not_schedule_extra_cleanup() {
    use futures_util::StreamExt;
    let kernel = hoh_kernel::Kernel::start();
    let ctx = kernel.root();
    let runtime = LlmRuntime::new();
    let count = Arc::new(AtomicUsize::new(0));
    runtime
        .register_adapter(
            &ctx,
            vec!["route".into()],
            Arc::new(CleanupAdapter(count.clone())),
        )
        .unwrap();
    let mut stream = runtime.stream(&ctx, options());
    stream.next().await.unwrap().unwrap();
    stream.close().await.unwrap();
    collect(runtime.stream(&ctx, options())).await.unwrap();
    drop(runtime.stream(&ctx, options()));
    tokio::task::yield_now().await;
    assert_eq!(count.load(Ordering::SeqCst), 1);
    kernel.shutdown().await;
}

#[tokio::test]
async fn exhausted_deferred_stream_does_not_schedule_return() {
    let (kernel, ctx, runtime, _) = setup().await;
    let count = Arc::new(AtomicUsize::new(0));
    let cleanup = count.clone();
    collect(runtime.stream(&ctx, options()).with_cleanup(move || {
        Box::pin(async move {
            cleanup.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
    }))
    .await
    .unwrap();
    tokio::task::yield_now().await;
    assert_eq!(count.load(Ordering::SeqCst), 0);
    kernel.shutdown().await;
}

#[tokio::test]
async fn cleanup_owner_reaches_streams_already_polled_by_middleware() {
    use futures_util::StreamExt;
    let kernel = hoh_kernel::Kernel::start();
    let ctx = kernel.root();
    let runtime = LlmRuntime::new();
    let count = Arc::new(AtomicUsize::new(0));
    runtime
        .register_adapter(
            &ctx,
            vec!["route".into()],
            Arc::new(CleanupAdapter(count.clone())),
        )
        .unwrap();
    ctx.on_waterfall::<LlmStreamEvent, _, _>(|_, next| async move {
        let mut stream = next.run().await?;
        stream.next().await.unwrap()?;
        Ok(stream)
    })
    .unwrap();
    let mut stream = runtime.stream(&ctx, options());
    stream.next().await.unwrap().unwrap();
    drop(stream);
    tokio::task::yield_now().await;
    assert_eq!(count.load(Ordering::SeqCst), 1);
    kernel.shutdown().await;
}
