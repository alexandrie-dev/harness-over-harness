//! Port of credentials-local/tests/drain.spec.ts: write-drain teardown.
//!
//! The source mocks `writeFileAtomic` with a gated promise; here the writer seam is gated the same
//! way, so the dispose-versus-queued-write race is fully deterministic.
mod support;

use std::sync::Arc;

use hoh_atomic_write::{WriteFileAtomicOptions, write_file_atomic};
use hoh_credentials::{ApiKeyRecord, CredentialRecord, GrantRecord};
use hoh_credentials_local::{DocumentWriter, Seams};
use serde_json::json;
use support::*;

/// A writer that waits for the gate to open, then performs the real atomic write.
fn gated_writer() -> (DocumentWriter, tokio::sync::watch::Sender<bool>) {
    let (open, gate) = tokio::sync::watch::channel(false);
    let writer: DocumentWriter = Arc::new(move |path, content| {
        let mut gate = gate.clone();
        Box::pin(async move {
            let _ = gate.wait_for(|open| *open).await;
            write_file_atomic(
                path,
                content,
                WriteFileAtomicOptions {
                    mode: 0o600,
                    dir_mode: Some(0o700),
                },
            )
            .await
        })
    });
    (writer, open)
}

#[tokio::test]
async fn lets_the_in_flight_write_land_and_fails_the_queued_one_after_disposal() {
    let dir = temp_dir();
    let (writer, open) = gated_writer();
    let booted = boot_seamed(
        config(&doc(&dir)),
        Seams {
            writer: Some(writer),
            watcher: None,
        },
    )
    .await;
    let service = booted.credentials();
    let (key, other) = (reference("DSH_CRED_DRAIN_A"), reference("DSH_CRED_DRAIN_B"));

    let first = tokio::spawn({
        let (service, key) = (service.clone(), key.clone());
        async move { service.set(&key, "one").await }
    });
    // Let the first task pass its liveness checks and park on the gate, so it is genuinely
    // in-flight when disposal begins.
    pause(50).await;
    let second = tokio::spawn({
        let (service, other) = (service.clone(), other.clone());
        async move { service.set(&other, "two").await }
    });
    pause(50).await;
    let disposal = tokio::spawn(booted.fiber.dispose());
    // Give the drain disposer its first turn (set closed) before opening the gate.
    pause(100).await;
    open.send(true).unwrap();
    disposal.await.unwrap();

    first.await.unwrap().unwrap();
    let failure = second.await.unwrap().unwrap_err();
    assert!(
        failure.to_string().contains("disposed before the queued"),
        "{failure}"
    );
    assert_eq!(
        resolved(&*service, "DSH_CRED_DRAIN_A").await,
        Some(("one".into(), "file".into()))
    );
    assert_eq!(resolved(&*service, "DSH_CRED_DRAIN_B").await, None);
    booted.kernel.shutdown().await;
}

#[tokio::test]
async fn fails_a_queued_record_write_after_disposal_on_the_same_terms() {
    let dir = temp_dir();
    let (writer, open) = gated_writer();
    let booted = boot_seamed(
        config(&doc(&dir)),
        Seams {
            writer: Some(writer),
            watcher: None,
        },
    )
    .await;
    let service = booted.credentials();
    let alpha = key("llm-drain", "alpha");
    let beta = key("llm-drain", "beta");
    let record = CredentialRecord::Grant(GrantRecord {
        payload: json!({"v": 1}),
    });

    let first = tokio::spawn({
        let (service, alpha, record) = (service.clone(), alpha.clone(), record.clone());
        async move {
            service
                .modify_record(
                    &alpha,
                    Box::new(move |_| Box::pin(async move { Ok(Some(record)) })),
                )
                .await
        }
    });
    pause(50).await;
    let queued_modify = tokio::spawn({
        let (service, beta) = (service.clone(), beta.clone());
        async move {
            service
                .modify_record(
                    &beta,
                    Box::new(|_| {
                        Box::pin(async {
                            Ok(Some(CredentialRecord::ApiKey(ApiKeyRecord::default())))
                        })
                    }),
                )
                .await
        }
    });
    let queued_delete = tokio::spawn({
        let (service, beta) = (service.clone(), beta.clone());
        async move { service.delete_record(&beta).await }
    });
    pause(50).await;
    let disposal = tokio::spawn(booted.fiber.dispose());
    pause(100).await;
    open.send(true).unwrap();
    disposal.await.unwrap();

    assert_eq!(first.await.unwrap().unwrap(), Some(record));
    for queued in [
        queued_modify.await.unwrap().unwrap_err(),
        queued_delete.await.unwrap().unwrap_err(),
    ] {
        assert!(
            queued.to_string().contains("disposed before the queued"),
            "{queued}"
        );
    }
    assert_eq!(service.read_record(&beta).await.unwrap(), None);
    booted.kernel.shutdown().await;
}
