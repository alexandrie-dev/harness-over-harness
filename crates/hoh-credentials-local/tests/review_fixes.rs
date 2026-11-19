//! Port of credentials-local/tests/review-fixes.spec.ts.
mod support;

use std::sync::{Arc, Mutex};

use hoh_credentials::CredentialRecord;
use hoh_credentials::{ApiKeyRecord, CredentialsReferenceUpdated};
use support::*;

#[tokio::test]
async fn folds_an_unobserved_external_edit_into_a_write_instead_of_overwriting_it() {
    let dir = temp_dir();
    let path = doc(&dir);
    let booted = boot(config(&path)).await;
    let seen = updates(&booted.ctx);
    let credentials = booted.credentials();
    let (alpha, beta) = (reference("DSH_REVIEW_ALPHA"), reference("DSH_REVIEW_BETA"));
    credentials.set(&alpha, "one").await.unwrap();
    // The external edit has landed on disk but no watcher reported it (watch is off — the same
    // blind spot as a debounce window or a missed event).
    write_credentials(
        &path,
        "version: 1\nrefs:\n  DSH_REVIEW_ALPHA: one\n  DSH_REVIEW_BETA: external\n",
    );
    credentials.set(&alpha, "two").await.unwrap();
    let text = read(&path);
    assert!(text.contains("DSH_REVIEW_BETA: external"), "{text}");
    assert!(text.contains("DSH_REVIEW_ALPHA: two"), "{text}");
    // The fold published the unobserved entry before the write's own commit.
    assert_eq!(
        snapshot(&seen),
        vec!["DSH_REVIEW_ALPHA", "DSH_REVIEW_BETA", "DSH_REVIEW_ALPHA"]
    );
    assert_eq!(
        credentials
            .resolve(&beta)
            .await
            .unwrap()
            .map(|r| (r.value, r.source)),
        Some(("external".into(), "file".into()))
    );
    booted.shutdown().await;
}

#[tokio::test]
async fn keeps_both_refs_when_two_providers_write_the_same_document_concurrently() {
    let dir = temp_dir();
    let path = doc(&dir);
    let first = boot(config(&path)).await;
    let second = boot(config(&path)).await;
    let (alpha, beta) = (reference("DSH_REVIEW_ALPHA"), reference("DSH_REVIEW_BETA"));
    let (a, b) = (first.credentials(), second.credentials());
    tokio::join!(
        async {
            for value in ["1", "2", "3"] {
                a.set(&alpha, value).await.unwrap();
            }
        },
        async {
            for value in ["1", "2", "3"] {
                b.set(&beta, value).await.unwrap();
            }
        }
    );
    let third = boot(config(&path)).await;
    let credentials = third.credentials();
    assert_eq!(
        resolved(&*credentials, "DSH_REVIEW_ALPHA").await,
        Some(("3".into(), "file".into()))
    );
    assert_eq!(
        resolved(&*credentials, "DSH_REVIEW_BETA").await,
        Some(("3".into(), "file".into()))
    );
    third.shutdown().await;
    second.shutdown().await;
    first.shutdown().await;
}

#[tokio::test]
async fn creates_the_credentials_directory_owner_only() {
    let dir = temp_dir();
    let home = dir.path().join("home");
    let booted = boot(config(&home.join(".credentials.yaml"))).await;
    booted
        .credentials()
        .set(&reference("DSH_REVIEW_ALPHA"), "one")
        .await
        .unwrap();
    assert_eq!(mode(&home), 0o700);
    booted.shutdown().await;
}

#[tokio::test]
async fn holds_every_writer_of_the_document_to_the_record_mutation_lock_wait() {
    let dir = temp_dir();
    let path = doc(&dir);
    let holder = boot(config(&path)).await;
    let contender = boot(config(&path)).await;
    let doomed = key("llm-pi-ai", "doomed");
    let slow_key = key("llm-pi-ai", "slow");
    let api = |key: &str| {
        CredentialRecord::ApiKey(ApiKeyRecord {
            key: Some(key.into()),
            env: None,
        })
    };
    let record = api("x");
    holder
        .credentials()
        .modify_record(
            &doomed,
            Box::new(move |_| Box::pin(async move { Ok(Some(record)) })),
        )
        .await
        .unwrap();
    let (entered_tx, entered) = tokio::sync::oneshot::channel::<()>();
    // The mutation holds the cross-process writer lock across a stand-in for an OAuth refresh round
    // trip — longer than withFileLock's 2s default.
    let slow_record = api("slow");
    let holding = holder.credentials();
    let slow_key_owned = slow_key.clone();
    let slow = tokio::spawn(async move {
        holding
            .modify_record(
                &slow_key_owned,
                Box::new(move |_| {
                    Box::pin(async move {
                        let _ = entered_tx.send(());
                        tokio::time::sleep(std::time::Duration::from_millis(2_400)).await;
                        Ok(Some(slow_record))
                    })
                }),
            )
            .await
    });
    entered.await.unwrap();
    // The other two writer paths — a reference write and a record delete — share that file and that
    // lock, so they must wait the refresh out rather than fail at the file-work default.
    let contender_credentials = contender.credentials();
    let alpha = reference("DSH_REVIEW_ALPHA");
    let (set, delete) = tokio::join!(
        contender_credentials.set(&alpha, "waited"),
        contender_credentials.delete_record(&doomed)
    );
    set.unwrap();
    delete.unwrap();
    slow.await.unwrap().unwrap();
    let reread = boot(config(&path)).await;
    let credentials = reread.credentials();
    assert_eq!(
        resolved(&*credentials, "DSH_REVIEW_ALPHA").await,
        Some(("waited".into(), "file".into()))
    );
    assert_eq!(credentials.read_record(&doomed).await.unwrap(), None);
    assert_eq!(
        credentials.read_record(&slow_key).await.unwrap(),
        Some(api("slow"))
    );
    reread.shutdown().await;
    contender.shutdown().await;
    holder.shutdown().await;
}

#[tokio::test]
async fn does_not_fail_a_committed_set_when_a_listener_fails_and_later_listeners_still_run() {
    let dir = temp_dir();
    let booted = boot(config(&doc(&dir))).await;
    booted
        .ctx
        .on_contained::<CredentialsReferenceUpdated>(|_| Err("observer boom".into()))
        .unwrap();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let sink = calls.clone();
    booted
        .ctx
        .on_contained::<CredentialsReferenceUpdated>(move |event| {
            sink.lock().unwrap().push(event.reference.to_string());
            Ok(())
        })
        .unwrap();
    let alpha = reference("DSH_REVIEW_ALPHA");
    booted.credentials().set(&alpha, "one").await.unwrap();
    assert_eq!(*calls.lock().unwrap(), vec!["DSH_REVIEW_ALPHA"]);
    assert_eq!(
        resolved(&*booted.credentials(), "DSH_REVIEW_ALPHA").await,
        Some(("one".into(), "file".into()))
    );
    assert!(
        warnings(&booted.ctx)
            .iter()
            .any(|message| message.contains("observer boom")),
        "{:?}",
        warnings(&booted.ctx)
    );
    booted.shutdown().await;
}

#[tokio::test]
async fn contains_an_async_listener_rejection() {
    let dir = temp_dir();
    let booted = boot(config(&doc(&dir))).await;
    booted
        .ctx
        .on_contained_async::<CredentialsReferenceUpdated, _>(|_| async {
            Err("async observer boom".into())
        })
        .unwrap();
    booted
        .credentials()
        .set(&reference("DSH_REVIEW_ALPHA"), "one")
        .await
        .unwrap();
    pause(50).await;
    booted.shutdown().await;
}

#[tokio::test]
async fn leaves_a_sibling_multi_line_value_untouched_while_patching_one_entry() {
    let dir = temp_dir();
    let path = doc(&dir);
    write_credentials(
        &path,
        "version: 1\nrefs:\n  DSH_REVIEW_WRAPPED: |-\n    line1\n    line2\n  DSH_REVIEW_ALPHA: a\n",
    );
    let booted = boot(config(&path)).await;
    booted
        .credentials()
        .set(&reference("DSH_REVIEW_ALPHA"), "b")
        .await
        .unwrap();
    assert_eq!(
        read(&path),
        "version: 1\nrefs:\n  DSH_REVIEW_WRAPPED: |-\n    line1\n    line2\n  DSH_REVIEW_ALPHA: b\n"
    );
    assert_eq!(
        resolved(&*booted.credentials(), "DSH_REVIEW_WRAPPED").await,
        Some(("line1\nline2".into(), "file".into()))
    );
    booted.shutdown().await;
}

#[tokio::test]
async fn stores_a_value_that_looks_like_another_entry_without_creating_one() {
    let dir = temp_dir();
    let path = doc(&dir);
    let booted = boot(config(&path)).await;
    // The stored text must stay a value: a quoted-scalar write that leaked its own structure would
    // silently mint a credential nobody stored.
    booted
        .credentials()
        .set(&reference("DSH_REVIEW_ALPHA"), "DSH_REVIEW_INNER: injected")
        .await
        .unwrap();
    let reread = boot(config(&path)).await;
    let credentials = reread.credentials();
    assert_eq!(
        resolved(&*credentials, "DSH_REVIEW_ALPHA").await,
        Some(("DSH_REVIEW_INNER: injected".into(), "file".into()))
    );
    assert_eq!(resolved(&*credentials, "DSH_REVIEW_INNER").await, None);
    reread.shutdown().await;
    booted.shutdown().await;
}
