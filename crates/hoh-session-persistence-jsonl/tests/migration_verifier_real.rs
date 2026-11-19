mod common;
use common::*;
use hoh_session_persistence_jsonl::{migration_verifier::*, worker::VerificationRequest, *};
use sha2::{Digest, Sha256};
#[tokio::test]
async fn naturally_exits_after_verifying_raw_and_zstd_input() {
    for compression in [JsonlCompression::None, JsonlCompression::Zstd] {
        let f = Fixture::new(compression).await;
        let (path, bytes) = f.store(&meta("natural-exit"), &[]).await;
        let s = VerificationScheduler::default();
        let result = s
            .verify(
                VerificationRequest {
                    path: path.clone(),
                    compression,
                    expected_id: "natural-exit".into(),
                    expected_event_count: 0,
                    expected_prefix: None,
                },
                None,
            )
            .await
            .unwrap();
        assert_eq!(result.bytes, bytes.len());
        let digest = Sha256::digest(&bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        assert_eq!(result.digest, digest);
        assert_eq!(tokio::fs::read(path).await.unwrap(), bytes);
        f.finish().await;
    }
}
#[tokio::test]
async fn rejects_malformed_worker_data_before_opening_the_file() {
    let scheduler = VerificationScheduler::default();
    for count in [0, usize::MAX] {
        let request = VerificationRequest {
            path: "/must-not-be-read".into(),
            compression: JsonlCompression::None,
            expected_id: "session".into(),
            expected_event_count: count,
            expected_prefix: if count == 0 {
                Some(
                    hoh_session_persistence_jsonl::generation::JsonlExpectedPrefix {
                        bytes: 0,
                        digest: "not a SHA-256 digest".into(),
                    },
                )
            } else {
                None
            },
        };
        fail(
            scheduler.verify(request, None).await,
            "migration verifier request is malformed",
        );
    }
}
