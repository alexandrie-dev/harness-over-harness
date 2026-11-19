mod common;
use common::*;
use hoh_session_persistence::*;
use hoh_session_persistence_jsonl::JsonlCompression;
use serde_json::{Value, json};
fn developer() -> Value {
    let mut e = event(
        "developer/message",
        4,
        json!({"turn":1,"step":1,"message":{"id":"developer","role":"developer","source":{"kind":"tool-registry"},"content":[],"extra":{"retained":true}}}),
    );
    e["surfaceOp"] = "append".into();
    e
}
fn prefix() -> Vec<Value> {
    vec![
        event("turn/start", 0, json!({"turn":1})),
        event("step/start", 1, json!({"turn":1,"step":1})),
        json!({"type":"system/message","seq":2,"time":3,"surfaceOp":"append","data":{"turn":1,"step":1,"message":{"id":"system","role":"system","source":{"kind":"system-prompt"},"content":[{"type":"text","text":"System"}]}}}),
        event(
            "request/header",
            3,
            json!({"reason":"initial","header":{"config":{"provider":"test","model":"test"},"tools":[{"name":"search","description":"Historical","parameters":{}}]}}),
        ),
    ]
}
async fn run(row: Value, corrupt_prefix: bool, expected: Option<&str>) {
    for compression in [JsonlCompression::None, JsonlCompression::Zstd] {
        for access in [SessionAccess::Read, SessionAccess::Write] {
            let f = Fixture::new(compression).await;
            let mut events = prefix();
            events.push(row.clone());
            let (path, mut bytes) = f.store(&meta("native-developer"), &events).await;
            if corrupt_prefix {
                let header = line(
                    &hoh_session_persistence_jsonl::format::to_header_line(
                        &meta("native-developer"),
                        None,
                    )
                    .unwrap(),
                );
                let mut lines = vec![header];
                lines.extend(prefix().iter().map(line));
                lines.push("{invalid json}\n".into());
                lines.push(line(&row));
                bytes = vec![];
                for l in lines {
                    if compression == JsonlCompression::None {
                        bytes.extend(l.bytes());
                    } else {
                        bytes.extend(
                            hoh_session_persistence_jsonl::zstd::compress_zstd_frame(l)
                                .await
                                .unwrap(),
                        );
                    }
                }
                tokio::fs::write(&path, &bytes).await.unwrap();
            }
            match expected {
                None => {
                    let h = f
                        .backend
                        .open(&"native-developer".into(), access, None)
                        .await
                        .unwrap();
                    assert_eq!(read(h.as_ref()).await, events);
                    h.close().await.unwrap();
                }
                Some(text) => {
                    let e = fail(
                        f.backend
                            .open(&"native-developer".into(), access, None)
                            .await,
                        text,
                    );
                    assert!(
                        has_class::<SessionPersistenceCorruptionError>(e.as_ref()),
                        "{e:?}"
                    );
                }
            }
            assert_eq!(tokio::fs::read(path).await.unwrap(), bytes);
            f.finish().await;
        }
    }
}
#[tokio::test]
async fn preserves_an_empty_developer_node_and_additional_metadata_on_a_valid_open_step() {
    for ignorable in [false, true] {
        let mut row = developer();
        if ignorable {
            row["ignorable"] = true.into();
        }
        run(row, false, None).await;
    }
}
#[tokio::test]
async fn reads_name_only_additions_bound_to_their_historical_header_without_copying_definitions() {
    let mut row = developer();
    row["data"]["headerSeq"] = 3.into();
    row["data"]["message"]["content"] = json!([{"type":"tool-removal","toolName":"old"},{"type":"tool-addition","toolName":"search"}]);
    run(row, false, None).await;
}
#[tokio::test]
async fn refuses_invalid_historical_schema_binding_without_rewriting_the_generation() {
    for (seq, name, text) in [
        (0, "search", "earlier request/header"),
        (4, "search", "must name an earlier event"),
        (3, "absent", "exactly one tool"),
    ] {
        let mut row = developer();
        row["data"]["headerSeq"] = seq.into();
        row["data"]["message"]["content"] = json!([{"type":"tool-addition","toolName":name}]);
        run(row, false, Some(text)).await;
    }
}
#[tokio::test]
async fn refuses_malformed_known_ignorable_developer_data_even_after_a_corrupt_row() {
    let mut row = developer();
    row["ignorable"] = true.into();
    row["data"]["message"]["content"] =
        json!([{"type":"tool-addition","toolName":"search","tool":null}]);
    run(row, true, Some("omit inline tool definitions")).await;
}
#[tokio::test]
async fn refuses_a_developer_event_with_a_mismatched_step_without_rewriting() {
    let mut row = developer();
    row["data"]["step"] = 2.into();
    run(row, false, Some("open turn and step")).await;
}
#[tokio::test]
async fn refuses_a_developer_event_shadowing_the_protected_system_head() {
    let mut row = developer();
    row["surfaceOp"] = json!({"op":"replace","startSeq":2,"endSeq":2});
    row["sourceEventSeqs"] = json!([2]);
    run(row, false, Some("protected system head")).await;
}
