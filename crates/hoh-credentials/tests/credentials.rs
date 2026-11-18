#[path = "support/memory.rs"]
mod memory;

use hoh_credentials::{
    CredentialInfo, CredentialRecord, Credentials, CredentialsRecordUpdated,
    CredentialsReferenceUpdated, GrantRecord, ResolvedCredential, credential_key, credential_ref,
    is_credential_key_segment,
};
use hoh_kernel::{Context, Kernel, Level, Levels, Record};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Notify;

async fn boot(seed: Value) -> (Kernel, Context) {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    ctx.plugin::<memory::MemoryCredentialsPlugin>(seed)
        .unwrap()
        .settled()
        .await
        .unwrap();
    (kernel, ctx)
}

#[test]
fn brands_posix_shell_identifiers() {
    for value in ["DEEPSEEK_API_KEY", "_private", "lower_case9"] {
        assert_eq!(credential_ref(value).unwrap(), value);
    }
}

#[test]
fn rejects_every_other_shape() {
    for invalid in ["", "9LEADING", "WITH-DASH", "WITH SPACE", "ns:key"] {
        assert_eq!(credential_ref(invalid).unwrap_err().name(), "TypeError");
    }
}

#[test]
fn answers_whether_credential_key_would_accept_the_segment() {
    for valid in ["llm-pi-ai", "openai-codex", "a", "z9"] {
        assert!(is_credential_key_segment(valid));
    }
    for invalid in ["", "My_Proxy", "z.ai", "UPPER", "9leading", "a/b"] {
        assert!(!is_credential_key_segment(invalid));
    }
}

#[tokio::test]
async fn mounts_as_ctx_credentials_and_resolves_a_seeded_reference_with_its_source() {
    let (kernel, ctx) = boot(json!({"DEEPSEEK_API_KEY":"sk-seeded"})).await;
    let provider = ctx.get::<Credentials>().unwrap();
    let reference = credential_ref("DEEPSEEK_API_KEY").unwrap();
    assert_eq!(
        provider.resolve(&reference).await.unwrap(),
        Some(ResolvedCredential {
            value: "sk-seeded".into(),
            source: "memory".into()
        })
    );
    assert_eq!(
        provider.describe(&reference).await.unwrap(),
        CredentialInfo {
            configured: true,
            source: Some("memory".into()),
            writable: true,
            masked: None,
        }
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn treats_an_empty_stored_value_as_absent_everywhere() {
    let (kernel, ctx) = boot(json!({"DEEPSEEK_API_KEY":""})).await;
    let provider = ctx.get::<Credentials>().unwrap();
    let reference = credential_ref("DEEPSEEK_API_KEY").unwrap();
    assert_eq!(provider.resolve(&reference).await.unwrap(), None);
    assert_eq!(
        provider.describe(&reference).await.unwrap(),
        CredentialInfo {
            configured: false,
            source: None,
            writable: true,
            masked: None
        }
    );
    kernel.shutdown().await;
}

#[tokio::test]
async fn stores_through_set_removes_through_unset_and_emits_the_committed_change() {
    let (kernel, ctx) = boot(json!({})).await;
    let provider = ctx.get::<Credentials>().unwrap();
    let reference = credential_ref("DEEPSEEK_API_KEY").unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let seen = events.clone();
    ctx.on_contained::<CredentialsReferenceUpdated>(move |event| {
        seen.lock().unwrap().push(event.reference.clone());
        Ok(())
    })
    .unwrap();
    provider.set(&reference, "sk-live").await.unwrap();
    assert_eq!(
        provider.resolve(&reference).await.unwrap(),
        Some(ResolvedCredential {
            value: "sk-live".into(),
            source: "memory".into()
        })
    );
    provider.unset(&reference).await.unwrap();
    assert_eq!(provider.resolve(&reference).await.unwrap(), None);
    assert_eq!(*events.lock().unwrap(), [reference.clone(), reference]);
    kernel.shutdown().await;
}

#[tokio::test]
async fn rejects_an_empty_set_and_keeps_an_absent_unset_silent() {
    let (kernel, ctx) = boot(json!({})).await;
    let provider = ctx.get::<Credentials>().unwrap();
    let reference = credential_ref("DEEPSEEK_API_KEY").unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let seen = events.clone();
    ctx.on_contained::<CredentialsReferenceUpdated>(move |event| {
        seen.lock().unwrap().push(event.reference.clone());
        Ok(())
    })
    .unwrap();
    assert!(
        provider
            .set(&reference, "")
            .await
            .unwrap_err()
            .to_string()
            .contains("empty value")
    );
    provider.unset(&reference).await.unwrap();
    assert!(events.lock().unwrap().is_empty());
    kernel.shutdown().await;
}

#[tokio::test]
async fn removes_the_service_with_its_fiber() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let fiber = ctx
        .plugin::<memory::MemoryCredentialsPlugin>(json!({}))
        .unwrap();
    fiber.settled().await.unwrap();
    assert!(ctx.get::<Credentials>().is_some());
    fiber.dispose().await;
    assert!(ctx.get::<Credentials>().is_none());
    kernel.shutdown().await;
}

fn warnings(ctx: &Context) -> Vec<Record> {
    ctx.logger()
        .buffer()
        .into_iter()
        .filter(|record| record.level == Level::Warn)
        .collect()
}

fn report_notice(ctx: &Context, message: &'static str) -> Arc<Notify> {
    let reported = Arc::new(Notify::new());
    let completed = reported.clone();
    ctx.logger()
        .exporter(Levels::default(), move |record| {
            if record.level == Level::Warn && record.message == message {
                completed.notify_one();
            }
        })
        .unwrap();
    reported
}

// credentials-local/tests/review-fixes.spec.ts, adapted to a seam-only provider.
#[tokio::test(start_paused = true)]
async fn does_not_fail_a_committed_set_when_a_listener_throws_and_later_listeners_still_run() {
    let (kernel, ctx) = boot(json!({})).await;
    let provider = ctx.get::<Credentials>().unwrap();
    let reference = credential_ref("DSH_REVIEW_ALPHA").unwrap();
    ctx.on_contained::<CredentialsReferenceUpdated>(|_| Err("observer boom".into()))
        .unwrap();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let seen = calls.clone();
    let log = ctx.logger();
    ctx.on_contained::<CredentialsReferenceUpdated>(move |event| {
        // Synchronous failure is reported before the next observer is called.
        assert_eq!(log.buffer().len(), 2);
        seen.lock().unwrap().push(event.reference.clone());
        Ok(())
    })
    .unwrap();
    provider.set(&reference, "one").await.unwrap();
    assert_eq!(
        calls.lock().unwrap().as_slice(),
        std::slice::from_ref(&reference)
    );
    assert_eq!(
        provider.resolve(&reference).await.unwrap(),
        Some(ResolvedCredential {
            value: "one".into(),
            source: "memory".into(),
        })
    );
    let records = warnings(&ctx);
    assert_eq!(
        records
            .iter()
            .map(|r| r.message.as_str())
            .collect::<Vec<_>>(),
        [
            "credentials: a credentials/reference-updated listener for \"DSH_REVIEW_ALPHA\" failed",
            "observer boom",
        ]
    );
    assert!(
        records
            .iter()
            .all(|record| record.name.as_ref() == "credentials")
    );
    kernel.shutdown().await;
}

// credentials-local/tests/review-fixes.spec.ts; a gate replaces its wall-clock delay.
#[tokio::test(start_paused = true)]
async fn contains_an_async_listener_rejection() {
    let (kernel, ctx) = boot(json!({})).await;
    let provider = ctx.get::<Credentials>().unwrap();
    let reference = credential_ref("DSH_REVIEW_ALPHA").unwrap();
    let release = Arc::new(Notify::new());
    let gate = release.clone();
    ctx.on_contained_async::<CredentialsReferenceUpdated, _>(move |_| {
        let gate = gate.clone();
        async move {
            gate.notified().await;
            Err("async observer boom".into())
        }
    })
    .unwrap();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let seen = calls.clone();
    ctx.on_contained::<CredentialsReferenceUpdated>(move |event| {
        seen.lock().unwrap().push(event.reference.clone());
        Ok(())
    })
    .unwrap();
    let reported = report_notice(&ctx, "async observer boom");
    tokio::time::timeout(Duration::from_millis(500), provider.set(&reference, "one"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        calls.lock().unwrap().as_slice(),
        std::slice::from_ref(&reference)
    );
    assert_eq!(
        provider.resolve(&reference).await.unwrap(),
        Some(ResolvedCredential {
            value: "one".into(),
            source: "memory".into(),
        })
    );
    assert!(warnings(&ctx).is_empty());
    release.notify_one();
    tokio::time::timeout(Duration::from_millis(500), reported.notified())
        .await
        .unwrap();
    assert_eq!(
        warnings(&ctx)
            .iter()
            .map(|r| r.message.as_str())
            .collect::<Vec<_>>(),
        [
            "credentials: a credentials/reference-updated listener for \"DSH_REVIEW_ALPHA\" failed",
            "async observer boom",
        ]
    );
    kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn contains_record_listener_failures_without_waiting_for_async_work() {
    let (kernel, ctx) = boot(json!({})).await;
    let provider = ctx.get::<Credentials>().unwrap();
    let key = credential_key("test-provider", "grant").unwrap();
    let order = Arc::new(Mutex::new(Vec::new()));
    let calls = order.clone();
    ctx.on_contained::<CredentialsRecordUpdated>(move |_| {
        calls.lock().unwrap().push("sync");
        Err("sync record boom".into())
    })
    .unwrap();
    let calls = order.clone();
    let release = Arc::new(Notify::new());
    let gate = release.clone();
    ctx.on_contained_async::<CredentialsRecordUpdated, _>(move |_| {
        calls.lock().unwrap().push("async");
        let gate = gate.clone();
        async move {
            gate.notified().await;
            Err("async record boom".into())
        }
    })
    .unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let subjects = seen.clone();
    let calls = order.clone();
    let log = ctx.logger();
    ctx.on_contained::<CredentialsRecordUpdated>(move |event| {
        assert_eq!(log.buffer().len(), 2);
        calls.lock().unwrap().push("success");
        subjects.lock().unwrap().push(event.key.clone());
        Ok(())
    })
    .unwrap();
    let reported = report_notice(&ctx, "async record boom");
    let record = CredentialRecord::Grant(GrantRecord {
        payload: json!({"token":"one"}),
    });
    let replacement = record.clone();
    let result = tokio::time::timeout(
        Duration::from_millis(500),
        provider.modify_record(
            &key,
            Box::new(move |_| Box::pin(async move { Ok(Some(replacement)) })),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(result, Some(record.clone()));
    assert_eq!(provider.read_record(&key).await.unwrap(), Some(record));
    assert_eq!(*order.lock().unwrap(), ["sync", "async", "success"]);
    assert_eq!(*seen.lock().unwrap(), [key]);
    assert_eq!(warnings(&ctx).len(), 2);
    release.notify_one();
    tokio::time::timeout(Duration::from_millis(500), reported.notified())
        .await
        .unwrap();
    assert_eq!(
        warnings(&ctx)
            .iter()
            .map(|r| r.message.as_str())
            .collect::<Vec<_>>(),
        [
            "credentials: a credentials/record-updated listener for \"test-provider/grant\" failed",
            "sync record boom",
            "credentials: a credentials/record-updated listener for \"test-provider/grant\" failed",
            "async record boom",
        ]
    );
    kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn record_notifications_follow_only_committed_replacements_and_deletions() {
    let (kernel, ctx) = boot(json!({})).await;
    let provider = ctx.get::<Credentials>().unwrap();
    let key = credential_key("test-provider", "empty").unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let seen = events.clone();
    ctx.on_contained::<CredentialsRecordUpdated>(move |event| {
        seen.lock().unwrap().push(event.key.clone());
        Ok(())
    })
    .unwrap();
    provider
        .modify_record(&key, Box::new(|_| Box::pin(async { Ok(None) })))
        .await
        .unwrap();
    provider.delete_record(&key).await.unwrap();
    assert!(events.lock().unwrap().is_empty());
    provider
        .modify_record(
            &key,
            Box::new(|_| {
                Box::pin(async { Ok(Some(CredentialRecord::ApiKey(Default::default()))) })
            }),
        )
        .await
        .unwrap();
    provider
        .modify_record(&key, Box::new(|_| Box::pin(async { Ok(None) })))
        .await
        .unwrap();
    provider.delete_record(&key).await.unwrap();
    provider.delete_record(&key).await.unwrap();
    assert_eq!(*events.lock().unwrap(), [key.clone(), key]);
    assert!(warnings(&ctx).is_empty());
    kernel.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn reference_notifications_ignore_the_provider_context_filter() {
    let kernel = Kernel::start();
    let root = kernel.root();
    root.with_filter(|_| false)
        .plugin::<memory::MemoryCredentialsPlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let seen = events.clone();
    root.on_contained::<CredentialsReferenceUpdated>(move |event| {
        seen.lock().unwrap().push(event.reference.clone());
        Ok(())
    })
    .unwrap();
    let provider = root.get::<Credentials>().unwrap();
    let reference = credential_ref("FILTERED_PROVIDER_KEY").unwrap();
    provider.set(&reference, "committed").await.unwrap();
    let resolved = provider.resolve(&reference).await.unwrap();
    let received = events.lock().unwrap().clone();
    kernel.shutdown().await;
    assert_eq!(
        resolved,
        Some(ResolvedCredential {
            value: "committed".into(),
            source: "memory".into(),
        })
    );
    assert_eq!(received.as_slice(), std::slice::from_ref(&reference));
}

#[tokio::test(start_paused = true)]
async fn record_notifications_ignore_the_provider_context_filter() {
    let kernel = Kernel::start();
    let root = kernel.root();
    root.with_filter(|_| false)
        .plugin::<memory::MemoryCredentialsPlugin>(json!({}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let seen = events.clone();
    root.on_contained::<CredentialsRecordUpdated>(move |event| {
        seen.lock().unwrap().push(event.key.clone());
        Ok(())
    })
    .unwrap();
    let provider = root.get::<Credentials>().unwrap();
    let key = credential_key("filtered-provider", "record").unwrap();
    let expected = CredentialRecord::Grant(GrantRecord {
        payload: json!({"token":"committed"}),
    });
    let replacement = expected.clone();
    let committed = provider
        .modify_record(
            &key,
            Box::new(move |_| Box::pin(async move { Ok(Some(replacement)) })),
        )
        .await
        .unwrap();
    let stored = provider.read_record(&key).await.unwrap();
    let received = events.lock().unwrap().clone();
    kernel.shutdown().await;
    assert_eq!(committed, Some(expected.clone()));
    assert_eq!(stored, Some(expected));
    assert_eq!(received.as_slice(), std::slice::from_ref(&key));
}
