mod common;
use common::*;
use hoh_session_persistence::*;
use hoh_session_persistence_jsonl::{JsonlCompression, catalog_migration::*, format::*};
use serde_json::{Value, json};

#[tokio::test]
async fn serves_the_released_v0_fixture_without_publishing_at_the_service_barrier() {
    for compression in [JsonlCompression::None, JsonlCompression::Zstd] {
        let f = Fixture::new(compression).await;
        // The source's migrationOneTurnLog(): step/start precedes the first
        // surface so V2 can add its protected system head without reordering.
        let header = json!({"type":"session","version":0,"id":"released-v0-read","createdAt":1000,"cwd":"/work","delegationDepth":0});
        let rows = vec![
            event("turn/start", 0, json!({"turn":1})),
            json!({"type":"step/start","seq":1,"time":2,"data":{"turn":1,"step":1}}),
            json!({"type":"user/message","seq":2,"time":3,"data":{"id":"one-turn-user","role":"user","content":[{"type":"text","text":"hi"}],"source":{"kind":"user"}},"surfaceOp":"append"}),
            json!({"type":"assistant/chunk","seq":3,"time":3,"data":{"turn":1,"step":1,"chunk":{"type":"block-start","index":0,"blockType":"text"}}}),
            json!({"type":"assistant/chunk","seq":4,"time":3,"data":{"turn":1,"step":1,"chunk":{"type":"text-delta","index":0,"text":"hello"}}}),
            json!({"type":"assistant/chunk","seq":5,"time":4,"data":{"turn":1,"step":1,"chunk":{"type":"block-end","index":0,"block":{"type":"text","text":"hello"}}}}),
            json!({"type":"assistant/chunk","seq":6,"time":4,"data":{"turn":1,"step":1,"chunk":{"type":"finish","reason":{"kind":"stop"}}}}),
            json!({"type":"assistant/message","seq":7,"time":4,"data":{"turn":1,"step":1,"message":{"id":"one-turn-assistant","role":"assistant","content":[{"type":"text","text":"hello"}],"source":{"kind":"model","provider":"mock","model":"mock"}}},"sourceEventSeqs":[3,4,5,6],"surfaceOp":"append"}),
            json!({"type":"step/end","seq":8,"time":5,"data":{"turn":1,"step":1}}),
            json!({"type":"turn/end","seq":9,"time":6,"data":{"turn":1,"reason":{"kind":"completed"}}}),
        ];
        let (path, bytes) = f.store_raw(&header, &rows).await;
        let catalog =
            hoh_session_format_catalog::create_session_format_catalog_with_children(vec![])
                .unwrap();
        let mut expected = catalog
            .create_restore(
                &header,
                hoh_session_format::SessionFormatRestoreOptions {
                    recovery: hoh_session_format::SessionFormatRecovery::Recoverable,
                    validation: hoh_session_format::SessionFormatValidation::Current,
                },
            )
            .unwrap();
        for row in &rows {
            expected.decode_row(row.clone()).unwrap();
        }
        let expected = expected.finish().unwrap();
        let id = "released-v0-read".into();
        let reader = f
            .backend
            .open(&id, SessionAccess::Read, None)
            .await
            .unwrap();
        assert_eq!(reader.header().version, 4);
        let restored = read(reader.as_ref()).await;
        assert_eq!(
            restored,
            expected
                .events
                .into_iter()
                .map(Value::Object)
                .collect::<Vec<_>>()
        );
        assert!(
            restored
                .iter()
                .any(|row| row["type"] == "assistant/message")
        );
        f.backend.flush().await.unwrap();
        assert_eq!(tokio::fs::read(&path).await.unwrap(), bytes);
        let current = generation_log_path(
            &f.root.path().to_string_lossy(),
            Some("/work"),
            &id,
            4,
            compression,
        )
        .unwrap();
        assert!(!tokio::fs::try_exists(&current).await.unwrap());
        reader.close().await.unwrap();
        let writer = f
            .backend
            .open(&id, SessionAccess::Write, None)
            .await
            .unwrap();
        assert_eq!(read(writer.as_ref()).await, restored);
        writer.close().await.unwrap();
        assert!(tokio::fs::try_exists(current).await.unwrap());
        assert_eq!(tokio::fs::read(path).await.unwrap(), bytes);
        f.finish().await;
    }
}

#[test]
fn refuses_fractional_foreign_version_before_header_shape() {
    let error = fail(
        scan_log(b"{\"version\":4.5,\"id\":\"x\"}\n"),
        "upgrade the harness",
    );
    assert!(has_class::<SessionFormatUnsupportedError>(error.as_ref()));
    assert_eq!(
        error.to_string(),
        "session \"x\" uses log format v4.5, but this harness reads only v4: the log was written by a newer harness — upgrade the harness to open it"
    );
}

#[tokio::test]
async fn refuses_malformed_and_duplicate_native_catalogs_without_migration_terminology() {
    for compression in [JsonlCompression::None, JsonlCompression::Zstd] {
        for duplicate in [false, true] {
            let f = Fixture::new(compression).await;
            let data = json!({"version":0,"childId":"child","childCreatedAt":2,"mode":if duplicate {"one-shot"}else{"invalid"}});
            let mut rows = vec![event("subagent/catalog", 0, data.clone())];
            if duplicate {
                rows.push(event("subagent/catalog", 1, data));
            }
            let (path, bytes) = f.store(&meta("catalog"), &rows).await;
            for access in [SessionAccess::Read, SessionAccess::Write] {
                let e = fail(
                    f.backend.open(&"catalog".into(), access, None).await,
                    if duplicate {
                        "duplicate catalog child child"
                    } else {
                        "supported versioned catalog fact"
                    },
                );
                assert!(has_class::<SessionPersistenceCorruptionError>(e.as_ref()));
                assert!(!e.to_string().contains("migration requires"));
                assert_eq!(tokio::fs::read(&path).await.unwrap(), bytes);
            }
            f.finish().await;
        }
    }
}

#[tokio::test]
async fn refuses_checkpoint_owned_by_a_different_compaction() {
    for compression in [JsonlCompression::None, JsonlCompression::Zstd] {
        let f = Fixture::new(compression).await;
        let user = |id: &str, source: Value| json!({"id":id,"role":"user","source":source,"content":[{"type":"text","text":"input"}]});
        let mut rows = [
            event("turn/start", 0, json!({"turn":1})),
            event("user/message", 1, user("input", json!({"kind":"user"}))),
            event(
                "compaction/start",
                2,
                json!({"compactionId":"owner","turn":1}),
            ),
            event(
                "compaction/summary",
                3,
                json!({"compactionId":"owner","summary":[{"type":"text","text":"Summary"}],"shadowedRange":{"start":1,"end":1},"shadowedSeqs":[1],"shadowedTokenCount":1,"provider":"mock","model":"mock"}),
            ),
            event(
                "user/message",
                4,
                user(
                    "checkpoint",
                    json!({"kind":"compact-checkpoint","compactionId":"wrong-owner"}),
                ),
            ),
            event(
                "compaction/end",
                5,
                json!({"compactionId":"owner","turn":1}),
            ),
            event(
                "turn/end",
                6,
                json!({"turn":1,"reason":{"kind":"completed"}}),
            ),
        ];
        rows[1]["surfaceOp"] = "append".into();
        rows[4]["surfaceOp"] = json!({"op":"replace","startSeq":1,"endSeq":1});
        rows[4]["sourceEventSeqs"] = json!([1, 2, 3]);
        let (path, bytes) = f.store(&meta("checkpoint"), &rows).await;
        fail(
            hoh_session_persistence_jsonl::generation::JsonlGenerationRuntime::default()
                .verify(&path, compression, "checkpoint", rows.len(), None)
                .await,
            "matching compaction/start",
        );
        for access in [SessionAccess::Read, SessionAccess::Write] {
            let e = fail(
                f.backend.open(&"checkpoint".into(), access, None).await,
                "matching compaction/start",
            );
            assert!(has_class::<SessionPersistenceCorruptionError>(e.as_ref()));
            assert_eq!(tokio::fs::read(&path).await.unwrap(), bytes);
        }
        f.finish().await;
    }
}

#[tokio::test]
async fn collects_healthy_descriptors_and_checks_every_identity_field() {
    for compression in [JsonlCompression::None, JsonlCompression::Zstd] {
        let f = Fixture::new(compression).await;
        let mut h = meta("child");
        h.version = 3;
        h.origin = Some("subagent".into());
        h.parent_session = Some("parent".into());
        h.delegation_depth = Some(0);
        let descriptor =
            json!({"version":3,"mode":"continuable","provider":"spawn","label":"old child"});
        let (path, _) = f
            .store(&h, &[event("subagent/descriptor", 0, descriptor.clone())])
            .await;
        let signal = hoh_timeout::AbortController::new().signal();
        let facts = prepare_catalog_facts(
            &"parent".into(),
            &[CatalogSource {
                header: h.clone(),
                path: path.clone(),
            }],
            compression,
            &signal,
        )
        .await
        .unwrap();
        assert!(facts.failures.is_empty());
        assert_eq!(facts.facts[0]["descriptorCount"], 1);
        assert_eq!(facts.facts[0]["descriptor"], descriptor);
        facts.validate().await.unwrap();
        for key in ["cwd", "isSeeded", "delegationDepth", "agentPreset"] {
            let mut expected = serde_json::to_value(&h).unwrap();
            expected[key] = match key {
                "isSeeded" => true.into(),
                "delegationDepth" => 1.into(),
                _ => "changed".into(),
            };
            let e = fail(
                prepare_catalog_facts(
                    &"parent".into(),
                    &[CatalogSource {
                        header: serde_json::from_value(expected).unwrap(),
                        path: path.clone(),
                    }],
                    compression,
                    &signal,
                )
                .await,
                "changed",
            );
            assert!(e.is::<hoh_session_persistence_jsonl::generation::JsonlGenerationSourceChangedError>());
        }
        f.finish().await;
    }
}

#[tokio::test]
async fn resumes_v0_through_v3_reads_without_publishing_then_publishes_on_write() {
    for compression in [JsonlCompression::None, JsonlCompression::Zstd] {
        for version in 0..=3 {
            let f = Fixture::new(compression).await;
            let header = if version <= 1 {
                json!({"version":version,"id":"historical","createdAt":1,"delegationDepth":0})
            } else {
                json!({"version":version,"id":"historical","createdAt":1,"isSeeded":false,"delegationDepth":0})
            };
            let rows = [
                event("turn/start", 0, json!({"turn":1})),
                event("step/start", 1, json!({"turn":1,"step":1})),
                event("step/end", 2, json!({"turn":1,"step":1})),
                event(
                    "turn/end",
                    3,
                    json!({"turn":1,"reason":{"kind":"completed"}}),
                ),
            ];
            let (source, bytes) = f.store_raw(&header, &rows).await;
            let reader = f
                .backend
                .open(&"historical".into(), SessionAccess::Read, None)
                .await
                .unwrap();
            assert_eq!(reader.header().version, 4);
            let before = read(reader.as_ref()).await;
            assert!(!before.is_empty());
            f.backend.flush().await.unwrap();
            assert!(!tokio::fs::try_exists(f.path("historical")).await.unwrap());
            assert_eq!(
                f.backend
                    .stat(&"historical".into(), None)
                    .await
                    .unwrap()
                    .unwrap()
                    .format_status,
                Some(SessionFormatStatus::MigrationRequired)
            );
            reader.close().await.unwrap();
            let writer = f
                .backend
                .open(&"historical".into(), SessionAccess::Write, None)
                .await
                .unwrap();
            assert_eq!(read(writer.as_ref()).await, before);
            writer.close().await.unwrap();
            assert!(tokio::fs::try_exists(f.path("historical")).await.unwrap());
            assert_eq!(tokio::fs::read(source).await.unwrap(), bytes);
            f.finish().await;
        }
    }
}

#[tokio::test]
async fn opens_a_historical_parent_independently_of_an_unrelated_future_generation() {
    for compression in [JsonlCompression::None, JsonlCompression::Zstd] {
        for access in [SessionAccess::Read, SessionAccess::Write] {
            let f = Fixture::new(compression).await;
            let (parent, parent_bytes) = f
                .store_raw(
                    &json!({"version":3,"id":"parent","createdAt":1,"isSeeded":false,"delegationDepth":0}),
                    &[],
                )
                .await;
            let (future, future_bytes) = f
                .store_raw(
                    &json!({"version":5,"id":"future-root","createdAt":2,"isSeeded":false,"delegationDepth":0}),
                    &[],
                )
                .await;
            let current = f.path("parent");
            let handle = f
                .backend
                .open(&"parent".into(), access, None)
                .await
                .unwrap();
            assert!(read(handle.as_ref()).await.is_empty());
            handle.close().await.unwrap();
            assert_eq!(tokio::fs::read(parent).await.unwrap(), parent_bytes);
            assert_eq!(tokio::fs::read(future).await.unwrap(), future_bytes);
            assert_eq!(
                tokio::fs::try_exists(current).await.unwrap(),
                access == SessionAccess::Write
            );
            f.finish().await;
        }
    }
}

#[tokio::test]
async fn backfills_missing_parent_catalog_from_descriptor_versions_one_to_three() {
    for compression in [JsonlCompression::None, JsonlCompression::Zstd] {
        for version in 1..=3 {
            let f = Fixture::new(compression).await;
            f.store_raw(&json!({"version":3,"id":"parent","createdAt":1,"isSeeded":false,"delegationDepth":0}),&[]).await;
            let mut descriptor = json!({"version":version,"provider":"spawn","label":"old child"});
            if version > 1 {
                descriptor["mode"] = "continuable".into();
            }
            f.store_raw(&json!({"version":3,"id":"child","createdAt":2,"isSeeded":false,"delegationDepth":0,"parentSession":"parent","origin":"subagent"}),&[event("subagent/descriptor",0,descriptor)]).await;
            let reader = f
                .backend
                .open(&"parent".into(), SessionAccess::Read, None)
                .await
                .unwrap();
            let rows = read(reader.as_ref()).await;
            assert_eq!(rows[0]["type"], "subagent/catalog");
            assert_eq!(rows[0]["data"]["label"], "old child");
            assert_eq!(rows[0]["data"]["childId"], "child");
            reader.close().await.unwrap();
            let writer = f
                .backend
                .open(&"parent".into(), SessionAccess::Write, None)
                .await
                .unwrap();
            assert_eq!(read(writer.as_ref()).await, rows);
            writer.close().await.unwrap();
            f.finish().await;
        }
    }
}
