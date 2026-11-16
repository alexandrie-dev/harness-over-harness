use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

use futures_util::FutureExt;
use hoh_deepseek_llm_api_extensions::{
    AcceptanceCallback, DeepSeekLlmApiExtensionRegistry, DeepSeekLlmApiExtensionRequest,
    DeepSeekLlmApiExtensions, ExtensionError, PreparedDeepSeekLlmApiExtension,
};
use hoh_error::{BoxError, SharedError};
use hoh_kernel::{Context, Kernel, Level};
use hoh_timeout::AbortController;
use serde::{Serialize, Serializer};
use serde_json::{Value, json};
use tokio::sync::Notify;

async fn harness() -> (Kernel, Context) {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let fiber = ctx
        .plugin::<DeepSeekLlmApiExtensionRegistry>(Value::Null)
        .unwrap();
    fiber.settled().await.unwrap();
    assert_eq!(fiber.name().as_ref(), "DeepSeekLlmApiExtensionRegistry");
    (kernel, ctx)
}

fn request() -> DeepSeekLlmApiExtensionRequest {
    DeepSeekLlmApiExtensionRequest {
        body: Default::default(),
        session_id: None,
        purpose: None,
        signal: AbortController::new().signal(),
    }
}

fn contribution(
    value: Value,
    accept: Option<AcceptanceCallback>,
) -> PreparedDeepSeekLlmApiExtension {
    PreparedDeepSeekLlmApiExtension { value, accept }
}

fn counter(count: &Arc<AtomicUsize>) -> AcceptanceCallback {
    let count = count.clone();
    Arc::new(move || {
        let count = count.clone();
        async move {
            count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        .boxed()
    })
}

#[derive(Clone)]
struct SharedJson(Arc<Mutex<Value>>);

impl Serialize for SharedJson {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.lock().unwrap().serialize(serializer)
    }
}

#[tokio::test(start_paused = true)]
async fn prepares_detached_fields_and_accepts_every_provider_exactly_once() {
    let (kernel, ctx) = harness().await;
    let registry = ctx.get::<DeepSeekLlmApiExtensions>().unwrap();
    let first = Arc::new(AtomicUsize::new(0));
    let second = Arc::new(AtomicUsize::new(0));
    let mutable = SharedJson(Arc::new(Mutex::new(json!({"value":"original"}))));
    let value = mutable.clone();
    let accept = counter(&first);
    registry
        .register(&ctx, "test_alpha", move |_| {
            let result = PreparedDeepSeekLlmApiExtension {
                value: value.clone(),
                accept: Some(accept.clone()),
            };
            async move { Ok::<_, BoxError>(Some(result)) }
        })
        .unwrap();
    let accept = counter(&second);
    registry
        .register(
            &ctx,
            "test_beta",
            move |request: Arc<DeepSeekLlmApiExtensionRequest>| {
                let accept = accept.clone();
                async move {
                    Ok::<_, BoxError>(Some(contribution(
                        json!([u64::from(request.body.contains_key("messages"))]),
                        Some(accept),
                    )))
                }
            },
        )
        .unwrap();
    let mut facts = request();
    facts.body.insert("messages".into(), json!([]));
    facts.session_id = Some("s".into());
    let prepared = registry.prepare(facts).await.unwrap();
    mutable.0.lock().unwrap()["value"] = json!("changed");
    assert_eq!(
        prepared.fields(),
        json!({"test_alpha":{"value":"original"},"test_beta":[1]})
            .as_object()
            .unwrap()
    );
    let (a, b) = tokio::join!(prepared.accept(), prepared.accept());
    a.unwrap();
    b.unwrap();
    prepared.accept().await.unwrap();
    assert_eq!(first.load(Ordering::SeqCst), 1);
    assert_eq!(second.load(Ordering::SeqCst), 1);
    kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn preserves_the_prepared_result_as_an_acceptance_method_receiver() {
    let (kernel, ctx) = harness().await;
    let registry = ctx.get::<DeepSeekLlmApiExtensions>().unwrap();
    struct ResultState {
        value: Value,
        accepted: u64,
    }
    let result = Arc::new(Mutex::new(ResultState {
        value: json!({"value":"receiver"}),
        accepted: 0,
    }));
    let state = result.clone();
    registry
        .register(&ctx, "test_alpha", move |_| {
            let state = state.clone();
            async move {
                let value = state.lock().unwrap().value.clone();
                let accept: AcceptanceCallback = Arc::new(move || {
                    state.lock().unwrap().accepted += 1;
                    std::future::ready(Ok(())).boxed()
                });
                Ok::<_, BoxError>(Some(contribution(value, Some(accept))))
            }
        })
        .unwrap();
    let prepared = registry.prepare(request()).await.unwrap();
    prepared.accept().await.unwrap();
    assert_eq!(result.lock().unwrap().accepted, 1);
    kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn rejects_duplicate_fields_and_releases_ownership_with_the_registering_fiber() {
    let (kernel, ctx) = harness().await;
    let registry = ctx.get::<DeepSeekLlmApiExtensions>().unwrap();
    let (send, receive) = tokio::sync::oneshot::channel();
    let send = Arc::new(Mutex::new(Some(send)));
    let owner = ctx
        .inject::<(), _>(move |ctx, ()| {
            assert!(send.lock().unwrap().take().unwrap().send(ctx).is_ok());
            async { Ok(()) }
        })
        .unwrap();
    owner.settled().await.unwrap();
    let owner_ctx = receive.await.unwrap();
    let dispose = registry
        .register(&owner_ctx, "test_alpha", |_| async {
            Ok::<_, BoxError>(Some(contribution(json!({"value":"one"}), None)))
        })
        .unwrap();
    let error = registry
        .register(&ctx, "test_alpha", |_| async {
            Ok::<_, BoxError>(Some(contribution(json!({"value":"two"}), None)))
        })
        .err()
        .unwrap();
    assert_eq!(
        error.to_string(),
        "deepseek-llm-api-extensions: field \"test_alpha\" is already registered"
    );
    dispose.dispose().await;
    registry
        .register(&ctx, "test_alpha", |_| async {
            Ok::<_, BoxError>(Some(contribution(json!({"value":"replacement"}), None)))
        })
        .unwrap();
    assert_eq!(
        registry.prepare(request()).await.unwrap().fields()["test_alpha"],
        json!({"value":"replacement"})
    );
    registry
        .register(&owner_ctx, "test_beta", |_| async {
            Ok::<_, BoxError>(Some(contribution(json!([2]), None)))
        })
        .unwrap();
    owner.dispose().await;
    assert!(
        !registry
            .prepare(request())
            .await
            .unwrap()
            .fields()
            .contains_key("test_beta")
    );
    kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn settles_every_acceptance_callback_before_reporting_one_or_several_failures() {
    let (kernel, ctx) = harness().await;
    let registry = ctx.get::<DeepSeekLlmApiExtensions>().unwrap();
    let later = Arc::new(AtomicUsize::new(0));
    registry
        .register(&ctx, "test_alpha", |_| async {
            let accept: AcceptanceCallback =
                Arc::new(|| async { Err::<(), BoxError>("alpha failed".into()) }.boxed());
            Ok::<_, BoxError>(Some(contribution(json!({"value":"x"}), Some(accept))))
        })
        .unwrap();
    let release = Arc::new(Notify::new());
    let started = Arc::new(Notify::new());
    let (later_clone, release_clone, started_clone) =
        (later.clone(), release.clone(), started.clone());
    registry
        .register(&ctx, "test_beta", move |_| {
            let (later, release, started) = (
                later_clone.clone(),
                release_clone.clone(),
                started_clone.clone(),
            );
            async move {
                let accept: AcceptanceCallback = Arc::new(move || {
                    let (later, release, started) =
                        (later.clone(), release.clone(), started.clone());
                    async move {
                        later.fetch_add(1, Ordering::SeqCst);
                        started.notify_one();
                        release.notified().await;
                        Err::<(), BoxError>("beta failed".into())
                    }
                    .boxed()
                });
                Ok::<_, BoxError>(Some(contribution(json!([2]), Some(accept))))
            }
        })
        .unwrap();
    let prepared = registry.prepare(request()).await.unwrap();
    let mut acceptance = Box::pin(prepared.accept());
    started.notified().await;
    assert!(
        std::future::poll_fn(|cx| {
            std::task::Poll::Ready(acceptance.as_mut().poll(cx).is_pending())
        })
        .await
    );
    release.notify_one();
    let error = acceptance.await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "DeepSeek LLM API extension acceptance failed"
    );
    match error.downcast_ref::<ExtensionError>().unwrap() {
        ExtensionError::Aggregate { errors } => assert_eq!(
            errors.iter().map(ToString::to_string).collect::<Vec<_>>(),
            ["alpha failed", "beta failed"]
        ),
        other => panic!("expected aggregate, got {other:?}"),
    }
    assert_eq!(later.load(Ordering::SeqCst), 1);
    assert!(error.ptr_eq(&prepared.accept().await.unwrap_err()));
    kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn reports_a_single_acceptance_failure_verbatim_and_omits_an_undefined_contribution() {
    let (kernel, ctx) = harness().await;
    let registry = ctx.get::<DeepSeekLlmApiExtensions>().unwrap();
    let failure = SharedError::new("single failure");
    let shared = failure.clone();
    registry
        .register(&ctx, "test_alpha", move |_| {
            let failure = shared.clone();
            async move {
                let accept: AcceptanceCallback = Arc::new(move || {
                    let failure = failure.clone();
                    async move { Err::<(), BoxError>(failure.into()) }.boxed()
                });
                Ok::<_, BoxError>(Some(contribution(json!({"value":"x"}), Some(accept))))
            }
        })
        .unwrap();
    registry
        .register(&ctx, "test_beta", |_| async {
            Ok::<Option<PreparedDeepSeekLlmApiExtension>, BoxError>(None)
        })
        .unwrap();
    let prepared = registry.prepare(request()).await.unwrap();
    assert_eq!(
        prepared.fields(),
        json!({"test_alpha":{"value":"x"}}).as_object().unwrap()
    );
    assert!(prepared.accept().await.unwrap_err().ptr_eq(&failure));
    assert!(prepared.accept().await.unwrap_err().ptr_eq(&failure));
    kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn rejects_invalid_field_names_and_omits_a_failing_field_warning_once() {
    let (kernel, ctx) = harness().await;
    let registry = ctx.get::<DeepSeekLlmApiExtensions>().unwrap();
    for field in ["", " ", " a", "a\n", "\u{feff}a", "a\u{00a0}"] {
        let error = registry
            .register(&ctx, field, |_| async {
                Ok::<_, BoxError>(Some(contribution(Value::Null, None)))
            })
            .err()
            .unwrap();
        assert_eq!(
            error.to_string(),
            "deepseek-llm-api-extensions: field must be a non-blank trimmed string"
        );
    }
    registry
        .register::<Value, _>(&ctx, "test_alpha", |_| async {
            Err::<Option<PreparedDeepSeekLlmApiExtension>, BoxError>("prepare failed".into())
        })
        .unwrap();
    let accepted = Arc::new(AtomicUsize::new(0));
    let accept = counter(&accepted);
    registry
        .register(&ctx, "test_beta", move |_| {
            let accept = accept.clone();
            async move { Ok::<_, BoxError>(Some(contribution(json!([1]), Some(accept)))) }
        })
        .unwrap();
    for _ in 0..2 {
        let prepared = registry.prepare(request()).await.unwrap();
        assert_eq!(
            prepared.fields(),
            json!({"test_beta":[1]}).as_object().unwrap()
        );
        prepared.accept().await.unwrap();
    }
    assert_eq!(accepted.load(Ordering::SeqCst), 2);
    let warnings: Vec<_> = ctx
        .logger()
        .buffer()
        .into_iter()
        .filter(|r| r.level == Level::Warn)
        .collect();
    assert_eq!(warnings.len(), 1);
    assert_eq!(
        warnings[0].message,
        "deepseek-llm-api-extensions: omitting field \"test_alpha\" from this request because its preparation failed: prepare failed"
    );
    assert_eq!(warnings[0].name.as_ref(), "deepseekLlmApiExtensions");
    kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn warns_once_with_the_error_display_and_its_complete_cause_chain_in_one_message() {
    let (kernel, ctx) = harness().await;
    let registry = ctx.get::<DeepSeekLlmApiExtensions>().unwrap();
    let failure = SharedError::new(
        hoh_error::HarnessError::new("prepare failed", "PREPARE").with_cause(
            hoh_error::HarnessError::new("could not read input", "READ")
                .with_cause("permission denied"),
        ),
    );
    registry
        .register::<Value, _>(&ctx, "test_alpha", move |_| {
            let failure = failure.clone();
            async move { Err::<Option<PreparedDeepSeekLlmApiExtension>, BoxError>(failure.into()) }
        })
        .unwrap();
    for _ in 0..2 {
        assert!(
            registry
                .prepare(request())
                .await
                .unwrap()
                .fields()
                .is_empty()
        );
    }
    let warnings: Vec<_> = ctx
        .logger()
        .buffer()
        .into_iter()
        .filter(|record| record.level == Level::Warn)
        .collect();
    assert_eq!(warnings.len(), 1);
    assert_eq!(warnings[0].name.as_ref(), "deepseekLlmApiExtensions");
    assert_eq!(
        warnings[0].message,
        "deepseek-llm-api-extensions: omitting field \"test_alpha\" from this request because its preparation failed: prepare failed: could not read input: permission denied"
    );
    kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn stops_waiting_for_a_provider_that_ignores_request_cancellation() {
    let (kernel, ctx) = harness().await;
    let registry = ctx.get::<DeepSeekLlmApiExtensions>().unwrap();
    let controller = AbortController::new();
    let started = Arc::new(Notify::new());
    let ready = started.clone();
    registry
        .register::<Value, _>(&ctx, "test_alpha", move |_| {
            ready.notify_one();
            std::future::pending::<Result<Option<PreparedDeepSeekLlmApiExtension>, BoxError>>()
        })
        .unwrap();
    let mut facts = request();
    facts.signal = controller.signal();
    let pending = registry.prepare(facts);
    started.notified().await;
    controller.abort_with("cancelled during extension preparation");
    let error = tokio::time::timeout(std::time::Duration::from_millis(500), pending)
        .await
        .unwrap()
        .err()
        .unwrap();
    assert!(
        SharedError::of(error.as_ref())
            .unwrap()
            .ptr_eq(&controller.signal().reason().unwrap())
    );
    kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn does_not_log_a_provider_failure_caused_by_request_cancellation() {
    let (kernel, ctx) = harness().await;
    let registry = ctx.get::<DeepSeekLlmApiExtensions>().unwrap();
    let controller = Arc::new(AbortController::new());
    let abort = controller.clone();
    registry
        .register::<Value, _>(
            &ctx,
            "test_alpha",
            move |request: Arc<DeepSeekLlmApiExtensionRequest>| {
                abort.abort_with("cancelled");
                async move {
                    request.signal.throw_if_aborted()?;
                    Ok::<Option<PreparedDeepSeekLlmApiExtension>, BoxError>(None)
                }
            },
        )
        .unwrap();
    let mut facts = request();
    facts.signal = controller.signal();
    let error = registry.prepare(facts).await.err().unwrap();
    assert!(
        SharedError::of(error.as_ref())
            .unwrap()
            .ptr_eq(&controller.signal().reason().unwrap())
    );
    assert!(
        ctx.logger()
            .buffer()
            .iter()
            .all(|record| record.level != Level::Warn)
    );
    kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn prepares_concurrently_preserves_registration_order_and_forwards_exact_request_facts() {
    let (kernel, ctx) = harness().await;
    let registry = ctx.get::<DeepSeekLlmApiExtensions>().unwrap();
    let gate = Arc::new(Notify::new());
    let release = gate.clone();
    registry
        .register(
            &ctx,
            "z_first",
            move |facts: Arc<DeepSeekLlmApiExtensionRequest>| {
                let release = release.clone();
                async move {
                    release.notified().await;
                    assert_eq!(facts.session_id.as_deref(), Some("s"));
                    assert_eq!(facts.purpose.as_deref(), Some("unfamiliar-purpose"));
                    assert_eq!(facts.body.get("messages"), Some(&json!([])));
                    Ok::<_, BoxError>(Some(contribution(json!({"nested":[1, true, null]}), None)))
                }
            },
        )
        .unwrap();
    registry
        .register(&ctx, "a_second", move |_| {
            gate.notify_one();
            async { Ok::<_, BoxError>(Some(contribution(json!(2), None))) }
        })
        .unwrap();
    let mut facts = request();
    facts.session_id = Some("s".into());
    facts.purpose = Some("unfamiliar-purpose".into());
    facts.body.insert("messages".into(), json!([]));
    let prepared = registry.prepare(facts).await.unwrap();
    assert_eq!(
        prepared
            .fields()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["z_first", "a_second"]
    );
    kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn prepared_acceptance_survives_owner_disposal_and_dropped_waiters() {
    let (kernel, ctx) = harness().await;
    let registry = ctx.get::<DeepSeekLlmApiExtensions>().unwrap();
    let accepted = Arc::new(AtomicUsize::new(0));
    let accept = counter(&accepted);
    let disposer = registry
        .register(&ctx, "test_alpha", move |_| {
            let accept = accept.clone();
            async move { Ok::<_, BoxError>(Some(contribution(json!(1), Some(accept)))) }
        })
        .unwrap();
    let prepared = registry.prepare(request()).await.unwrap();
    disposer.dispose().await;
    assert!(
        registry
            .prepare(request())
            .await
            .unwrap()
            .fields()
            .is_empty()
    );
    kernel.shutdown().await;
    drop(prepared.accept());
    tokio::task::yield_now().await;
    assert_eq!(accepted.load(Ordering::SeqCst), 1);
    prepared.accept().await.unwrap();
    assert_eq!(accepted.load(Ordering::SeqCst), 1);
}

#[tokio::test(start_paused = true)]
async fn rejects_an_already_aborted_request_without_starting_providers() {
    let (kernel, ctx) = harness().await;
    let registry = ctx.get::<DeepSeekLlmApiExtensions>().unwrap();
    let started = Arc::new(AtomicUsize::new(0));
    let called = started.clone();
    registry
        .register(&ctx, "test_alpha", move |_| {
            called.fetch_add(1, Ordering::SeqCst);
            async { Ok::<_, BoxError>(Some(contribution(Value::Null, None))) }
        })
        .unwrap();
    let controller = AbortController::new();
    controller.abort_with("already cancelled");
    let mut facts = request();
    facts.signal = controller.signal();
    let error = registry.prepare(facts).await.err().unwrap();
    assert!(
        SharedError::of(error.as_ref())
            .unwrap()
            .ptr_eq(&controller.signal().reason().unwrap())
    );
    assert_eq!(started.load(Ordering::SeqCst), 0);
    kernel.shutdown().await;
}

struct Unserializable;
impl Serialize for Unserializable {
    fn serialize<S: Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
        Err(serde::ser::Error::custom("cannot clone contribution"))
    }
}

#[tokio::test(start_paused = true)]
async fn omits_uncloneable_values_and_does_not_reset_warning_history_on_reregistration() {
    let (kernel, ctx) = harness().await;
    let registry = ctx.get::<DeepSeekLlmApiExtensions>().unwrap();
    let register = || {
        registry
            .register(&ctx, "test_alpha", |_| async {
                Ok::<_, BoxError>(Some(PreparedDeepSeekLlmApiExtension {
                    value: Unserializable,
                    accept: None,
                }))
            })
            .unwrap()
    };
    let dispose = register();
    assert!(
        registry
            .prepare(request())
            .await
            .unwrap()
            .fields()
            .is_empty()
    );
    dispose.dispose().await;
    register();
    assert!(
        registry
            .prepare(request())
            .await
            .unwrap()
            .fields()
            .is_empty()
    );
    assert_eq!(
        ctx.logger()
            .buffer()
            .iter()
            .filter(|r| r.level == Level::Warn)
            .count(),
        1
    );
    kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn quotes_duplicate_names_as_json_and_retains_non_trim_whitespace() {
    let (kernel, ctx) = harness().await;
    let registry = ctx.get::<DeepSeekLlmApiExtensions>().unwrap();
    let field = "\u{0085}a\"\\b";
    let register = || {
        registry.register(&ctx, field, |_| async {
            Ok::<_, BoxError>(Some(contribution(Value::Null, None)))
        })
    };
    register().unwrap();
    assert_eq!(
        register().err().unwrap().to_string(),
        "deepseek-llm-api-extensions: field \"\u{0085}a\\\"\\\\b\" is already registered"
    );
    kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn enumerates_integer_fields_like_a_javascript_object_without_reordering_acceptance() {
    let (kernel, ctx) = harness().await;
    let registry = ctx.get::<DeepSeekLlmApiExtensions>().unwrap();
    let accepted = Arc::new(Mutex::new(Vec::new()));
    for field in ["z", "2", "1", "01", "4294967295", "a"] {
        let accepted = accepted.clone();
        registry
            .register(&ctx, field, move |_| {
                let accepted = accepted.clone();
                async move {
                    let accept: AcceptanceCallback = Arc::new(move || {
                        accepted.lock().unwrap().push(field);
                        std::future::ready(Ok(())).boxed()
                    });
                    Ok::<_, BoxError>(Some(contribution(json!(1), Some(accept))))
                }
            })
            .unwrap();
    }
    let prepared = registry.prepare(request()).await.unwrap();
    assert_eq!(
        prepared
            .fields()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["1", "2", "z", "01", "4294967295", "a"]
    );
    prepared.accept().await.unwrap();
    assert_eq!(
        *accepted.lock().unwrap(),
        ["z", "2", "1", "01", "4294967295", "a"]
    );
    kernel.shutdown().await;
}
