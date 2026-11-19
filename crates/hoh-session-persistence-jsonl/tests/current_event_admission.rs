mod common;
use common::*;
use hoh_session_persistence::*;
use hoh_session_persistence_jsonl::{JsonlCompression, format::*};
use serde_json::json;
#[tokio::test]
async fn refuses_unknown_required_metadata_without_truncating_a_provider_append() {
    for metadata in [json!({"surfaceOp":"append"}), json!({"sourceEventSeqs":[]})] {
        let f = Fixture::new(JsonlCompression::None).await;
        let mut row = event("future/required", 0, json!({}));
        row.as_object_mut()
            .unwrap()
            .extend(metadata.as_object().unwrap().clone());
        let (path, bytes) = f.store(&meta("unknown"), std::slice::from_ref(&row)).await;
        assert_eq!(scan_log(&bytes).unwrap().events[0].as_value(), &row);
        use std::os::unix::fs::MetadataExt;
        let before = tokio::fs::metadata(&path).await.unwrap();
        for access in [SessionAccess::Read, SessionAccess::Write] {
            fail(
                f.backend.open(&"unknown".into(), access, None).await,
                "unknown to this harness and not marked ignorable",
            );
            assert_eq!(tokio::fs::read(&path).await.unwrap(), bytes);
            let after = tokio::fs::metadata(&path).await.unwrap();
            assert_eq!((after.dev(), after.ino()), (before.dev(), before.ino()));
        }
        f.finish().await;
    }
}
#[tokio::test]
async fn retains_ignorable_obsolete_types_through_scanning_and_a_provider_append() {
    for kind in ["tool/code-dispatch-start", "tool/code-dispatch"] {
        let f = Fixture::new(JsonlCompression::None).await;
        let start = event("turn/start", 0, json!({"turn":1}));
        let mut obsolete = event(
            kind,
            1,
            json!({"rootCallId":"root","parentCallId":"root","subCallId":"child","name":"read","arguments":{}}),
        );
        obsolete["ignorable"] = true.into();
        let (path, bytes) = f
            .store(&meta("obsolete"), &[start.clone(), obsolete.clone()])
            .await;
        assert_eq!(scan_log(&bytes).unwrap().events.len(), 2);
        let w = f
            .backend
            .open(&"obsolete".into(), SessionAccess::Write, None)
            .await
            .unwrap();
        assert_eq!(read(w.as_ref()).await, vec![start, obsolete]);
        w.append(
            &arcs(&[event(
                "turn/end",
                2,
                json!({"turn":1,"reason":{"kind":"completed"}}),
            )]),
            None,
        )
        .await
        .unwrap();
        w.close().await.unwrap();
        assert!(tokio::fs::read(path).await.unwrap().starts_with(&bytes));
        f.finish().await;
    }
}
