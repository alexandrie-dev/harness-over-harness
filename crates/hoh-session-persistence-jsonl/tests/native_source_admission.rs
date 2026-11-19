mod common;
use common::*;
use hoh_session_persistence::{
    SessionAccess, SessionPersistence, SessionPersistenceCorruptionError,
};
use hoh_session_persistence_jsonl::JsonlCompression;
use serde_json::{Value, json};
async fn run(source: Value, kind: &str, ignorable: bool, valid: bool) {
    for compression in [JsonlCompression::None, JsonlCompression::Zstd] {
        for access in [SessionAccess::Read, SessionAccess::Write] {
            let f = Fixture::new(compression).await;
            let mut row = event(
                kind,
                0,
                json!({"target":"next-turn","start":0,"inserted":[{"id":"input","role":"user","source":source,"content":[{"type":"text","text":"queued"}]}]}),
            );
            if ignorable {
                row["ignorable"] = true.into();
            }
            let (path, bytes) = f
                .store(&meta("native-source"), std::slice::from_ref(&row))
                .await;
            if valid {
                let h = f
                    .backend
                    .open(&"native-source".into(), access, None)
                    .await
                    .unwrap();
                assert_eq!(read(h.as_ref()).await, vec![row.clone()]);
                h.close().await.unwrap();
            } else {
                let e = fail(
                    f.backend.open(&"native-source".into(), access, None).await,
                    "producer-owned source kind",
                );
                assert!(has_class::<SessionPersistenceCorruptionError>(e.as_ref()));
            }
            assert_eq!(tokio::fs::read(&path).await.unwrap(), bytes);
            let mut names = tokio::fs::read_dir(std::path::Path::new(&path).parent().unwrap())
                .await
                .unwrap();
            let mut count = 0;
            while let Some(entry) = names.next_entry().await.unwrap() {
                if entry.file_name() != "session.lock" {
                    count += 1;
                }
            }
            assert_eq!(count, 1);
            f.finish().await;
        }
    }
}
#[tokio::test]
async fn refuses_malformed_queued_source_null() {
    run(Value::Null, "agent/inbox/spliced", false, false).await;
}
#[tokio::test]
async fn refuses_malformed_queued_source_object() {
    run(json!({}), "agent/inbox/spliced", false, false).await;
}
#[tokio::test]
async fn refuses_malformed_queued_source_array() {
    run(json!([]), "agent/inbox/spliced", false, false).await;
}
#[tokio::test]
async fn refuses_malformed_queued_source_empty_kind() {
    run(json!({"kind":""}), "agent/inbox/spliced", false, false).await;
}
#[tokio::test]
async fn refuses_malformed_queued_source_numeric_kind() {
    run(json!({"kind":1}), "agent/inbox/spliced", false, false).await;
}
#[tokio::test]
async fn still_validates_known_queued_events_marked_ignorable() {
    run(json!({}), "agent/inbox/spliced", true, false).await;
}
#[tokio::test]
async fn preserves_external_attribution_and_own_json_metadata_without_a_producer_installed() {
    run(hoh_util_values::js_json_parse("{\"kind\":\"external-producer\",\"__proto__\":{\"retain\":true},\"metadata\":{\"count\":1}}").unwrap(),"agent/inbox/spliced",false,true).await;
}
#[tokio::test]
async fn leaves_unknown_ignorable_queued_payloads_opaque() {
    run(Value::Null, "external/inbox", true, true).await;
}
