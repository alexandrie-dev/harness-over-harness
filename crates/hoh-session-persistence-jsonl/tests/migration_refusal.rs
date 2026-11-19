mod common;
use common::*;
use hoh_session_persistence::*;
use hoh_session_persistence_jsonl::{
    JsonlCompression, format::to_header_line, zstd::compress_zstd_frame,
};
use serde_json::json;
#[tokio::test]
async fn keeps_retired_required_dispatch_types_unsupported_beyond_a_damaged_native_suffix() {
    for compression in [JsonlCompression::None, JsonlCompression::Zstd] {
        for access in [SessionAccess::Read, SessionAccess::Write] {
            for kind in ["tool/code-dispatch-start", "tool/code-dispatch"] {
                let f = Fixture::new(compression).await;
                let (path, _) = f.store(&meta("retired-required"), &[]).await;
                let lines = [
                    line(&to_header_line(&meta("retired-required"), None).unwrap()),
                    line(&event("turn/start", 0, json!({"turn":1}))),
                    "{}\n".into(),
                    line(&event(kind, 2, json!({}))),
                ];
                let mut bytes = vec![];
                for l in lines {
                    if compression == JsonlCompression::None {
                        bytes.extend(l.bytes());
                    } else {
                        bytes.extend(compress_zstd_frame(l).await.unwrap());
                    }
                }
                tokio::fs::write(&path, &bytes).await.unwrap();
                let e = fail(
                    f.backend
                        .open(&"retired-required".into(), access, None)
                        .await,
                    kind,
                );
                assert!(has_class::<SessionFormatUnsupportedError>(e.as_ref()));
                assert_eq!(tokio::fs::read(path).await.unwrap(), bytes);
                f.finish().await;
            }
        }
    }
}
#[tokio::test]
async fn refuses_a_native_tool_result_without_its_advertised_call_before_exposing_a_handle() {
    for compression in [JsonlCompression::None, JsonlCompression::Zstd] {
        let f = Fixture::new(compression).await;
        let rows = vec![
            event("turn/start", 0, json!({"turn":1})),
            event("step/start", 1, json!({"turn":1,"step":1})),
            json!({"type":"tool/result","seq":2,"time":3,"surfaceOp":"append","data":{"turn":1,"step":1,"message":{"id":"orphan","role":"tool","toolCallId":"missing","source":{"kind":"tool","callId":"missing"},"content":[{"type":"text","text":"orphan result"}]}}}),
        ];
        let (path, bytes) = f.store(&meta("orphan"), &rows).await;
        for access in [SessionAccess::Read, SessionAccess::Write] {
            assert!(has_class::<SessionPersistenceCorruptionError>(
                fail(
                    f.backend.open(&"orphan".into(), access, None).await,
                    "advertised tool lifecycle"
                )
                .as_ref()
            ));
            assert_eq!(tokio::fs::read(&path).await.unwrap(), bytes);
        }
        f.finish().await;
    }
}
