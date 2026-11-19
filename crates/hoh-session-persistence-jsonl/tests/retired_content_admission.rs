mod common;
use common::*;
use hoh_session_persistence::*;
use hoh_session_persistence_jsonl::{JsonlCompression, format::*};
use serde_json::json;
#[tokio::test]
async fn refuses_v4_retired_content_even_after_a_corrupt_prefix_and_preserves_the_only_generation()
{
    for compression in [JsonlCompression::None, JsonlCompression::Zstd] {
        for access in [SessionAccess::Read, SessionAccess::Write] {
            for corrupt in [false, true] {
                let f = Fixture::new(compression).await;
                let row = json!({"type":"user/message","seq":0,"time":1,"surfaceOp":"append","data":{"id":"user","role":"user","source":{"kind":"user"},"content":[{"type":"tool-result","toolCallId":"retired-call","content":[{"type":"text","text":"result"}]}]}});
                let (path, _) = f.store(&meta("retired"), &[]).await;
                let mut bytes = vec![];
                for l in [
                    line(&to_header_line(&meta("retired"), None).unwrap()),
                    if corrupt {
                        "{invalid json}\n".into()
                    } else {
                        String::new()
                    },
                    line(&row),
                ] {
                    if l.is_empty() {
                        continue;
                    }
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
                assert!(has_class::<SessionPersistenceCorruptionError>(
                    fail(
                        f.backend.open(&"retired".into(), access, None).await,
                        "released tool-result wrapper"
                    )
                    .as_ref()
                ));
                assert_eq!(tokio::fs::read(path).await.unwrap(), bytes);
                f.finish().await;
            }
        }
    }
}
#[tokio::test]
async fn preserves_an_unknown_ignorable_v4_payload_without_interpreting_its_content() {
    for compression in [JsonlCompression::None, JsonlCompression::Zstd] {
        for access in [SessionAccess::Read, SessionAccess::Write] {
            let f = Fixture::new(compression).await;
            let row = json!({"type":"future/opaque","seq":0,"time":1,"ignorable":true,"data":{"content":[{"type":"tool-result","toolCallId":"retired-call","content":[]}]}});
            let (path, bytes) = f.store(&meta("opaque"), std::slice::from_ref(&row)).await;
            let h = f
                .backend
                .open(&"opaque".into(), access, None)
                .await
                .unwrap();
            assert_eq!(read(h.as_ref()).await, vec![row]);
            h.close().await.unwrap();
            assert_eq!(tokio::fs::read(path).await.unwrap(), bytes);
            f.finish().await;
        }
    }
}
