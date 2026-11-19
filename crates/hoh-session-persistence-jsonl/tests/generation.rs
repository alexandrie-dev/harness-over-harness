mod common;
use common::*;
use hoh_error::BoxError;
use hoh_session_format::{SessionFormatArtifact, SessionFormatEvent, SessionFormatHeader};
use hoh_session_persistence_jsonl::{
    format::*,
    generation::*,
    testing::{IoHook, IoOperation},
    *,
};
use serde_json::{Value, json};
use std::{
    os::unix::fs::MetadataExt,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
struct Restore {
    header: SessionFormatHeader,
    events: Vec<SessionFormatEvent>,
}
impl JsonlGenerationRestore for Restore {
    fn decode_row(&mut self, value: Value) -> Result<()> {
        self.events.push(
            value
                .as_object()
                .ok_or_else(|| Box::new(std::io::Error::other("row must be object")) as BoxError)?
                .clone(),
        );
        Ok(())
    }
    fn finish(self: Box<Self>) -> Result<SessionFormatArtifact> {
        Ok(SessionFormatArtifact {
            header: self.header,
            inherited_event_count: 0,
            events: self.events,
        })
    }
}
#[derive(Default)]
struct Format {
    created: AtomicUsize,
    bad_target: bool,
    encode_failure: bool,
}
impl JsonlGenerationFormatAdapter for Format {
    fn current_version(&self) -> u64 {
        4
    }
    fn create_restore(&self, header: &Value) -> Result<Box<dyn JsonlGenerationRestore>> {
        self.created.fetch_add(1, Ordering::SeqCst);
        let mut header = header.as_object().unwrap().clone();
        header.shift_remove("type");
        header.insert(
            "version".into(),
            if self.bad_target { 5.into() } else { 4.into() },
        );
        Ok(Box::new(Restore {
            header,
            events: vec![],
        }))
    }
    fn encode_header(&self, header: &SessionFormatHeader, _cut: u64) -> Result<Value> {
        let mut out = serde_json::Map::new();
        out.insert("type".into(), "session".into());
        out.extend(header.clone());
        Ok(Value::Object(out))
    }
    fn encode_event(&self, event: &SessionFormatEvent) -> Result<Value> {
        if self.encode_failure {
            return Err(Box::new(std::io::Error::other("event encoder failed")));
        }
        Ok(Value::Object(event.clone()))
    }
}
async fn setup(
    compression: JsonlCompression,
) -> (Fixture, PrepareJsonlMigrationOptions, Arc<Format>, Vec<u8>) {
    let f = Fixture::new(compression).await;
    let mut h = serde_json::to_value(meta("generation")).unwrap();
    h["version"] = 3.into();
    let (source, bytes) = f.store_raw(&h, &log()).await;
    let format = Arc::new(Format::default());
    let verify: VerifyCurrentFile = Arc::new(move |path, c, id, n, prefix, _| {
        Box::pin(
            async move { verify_jsonl_current_generation(&path, c, &id, n, prefix.as_ref()).await },
        )
    });
    let opts = PrepareJsonlMigrationOptions {
        source_path: source,
        current_path: f.path("generation"),
        source_version: 3,
        compression,
        format: format.clone(),
        verify_current_file: verify,
        validate_related_sources: None,
        validate_historical_header: None,
        signal: None,
    };
    (f, opts, format, bytes)
}
async fn names(path: &str) -> Vec<String> {
    let mut entries = tokio::fs::read_dir(std::path::Path::new(path).parent().unwrap())
        .await
        .unwrap();
    let mut result = vec![];
    while let Some(e) = entries.next_entry().await.unwrap() {
        result.push(e.file_name().to_str().unwrap().to_string());
    }
    result.sort();
    result
}
#[tokio::test]
async fn prepares_read_only_then_publishes_one_current_generation_without_rewriting_the_source() {
    for compression in [JsonlCompression::None, JsonlCompression::Zstd] {
        let (f, o, format, bytes) = setup(compression).await;
        let before = tokio::fs::metadata(&o.source_path).await.unwrap();
        let p = prepare_jsonl_migration(o.clone()).await.unwrap();
        assert_eq!(p.artifact.events.len(), 6);
        assert!(!std::path::Path::new(&o.current_path).exists());
        let (a, b) = tokio::join!(p.publish(), p.publish());
        assert_eq!(a.unwrap(), b.unwrap());
        assert_eq!(format.created.load(Ordering::SeqCst), 1);
        let verified =
            verify_jsonl_current_generation(&o.current_path, compression, "generation", 6, None)
                .await
                .unwrap();
        assert!(verified.bytes > 0);
        assert_eq!(tokio::fs::read(&o.source_path).await.unwrap(), bytes);
        let after = tokio::fs::metadata(&o.source_path).await.unwrap();
        assert_eq!(
            (before.dev(), before.ino(), before.mtime_nsec()),
            (after.dev(), after.ino(), after.mtime_nsec())
        );
        assert_eq!(names(&o.source_path).await.len(), 2);
        f.finish().await;
    }
}
#[tokio::test]
async fn writes_one_header_frame_and_one_body_frame_for_a_compressed_generation() {
    let (f, o, _, _) = setup(JsonlCompression::Zstd).await;
    let p = prepare_jsonl_migration(o.clone()).await.unwrap();
    p.publish().await.unwrap();
    let bytes = tokio::fs::read(&o.current_path).await.unwrap();
    let frames = zstd::scan_zstd_frames(&bytes, None).unwrap();
    assert_eq!(frames.frames.len(), 2);
    assert!(frames.torn_start.is_none());
    let head = zstd::decompress_zstd_frame(&bytes[frames.frames[0].start..frames.frames[0].end])
        .await
        .unwrap();
    assert_eq!(head.iter().filter(|b| **b == b'\n').count(), 1);
    f.finish().await;
}
#[tokio::test]
async fn fails_a_stale_publication_once_without_re_running_the_migration() {
    let (f, o, format, _) = setup(JsonlCompression::None).await;
    let p = prepare_jsonl_migration(o.clone()).await.unwrap();
    append_bytes(&o.source_path, b" ").await;
    let a = fail(p.publish().await, "changed during migration");
    assert!(has_class::<JsonlGenerationSourceChangedError>(a.as_ref()));
    let b = fail(p.publish().await, "changed during migration");
    let a = hoh_error::SharedError::of(a.as_ref()).unwrap();
    let b = hoh_error::SharedError::of(b.as_ref()).unwrap();
    assert!(a.ptr_eq(b));
    assert_eq!(format.created.load(Ordering::SeqCst), 1);
    assert_eq!(names(&o.source_path).await.len(), 1);
    f.finish().await;
}
#[tokio::test]
async fn validates_related_sources_at_preparation_and_immediately_before_publication() {
    let (f, mut o, _, _) = setup(JsonlCompression::None).await;
    let calls = Arc::new(AtomicUsize::new(0));
    let n = calls.clone();
    o.validate_related_sources = Some(Arc::new(move || {
        let n = n.clone();
        Box::pin(async move {
            n.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
    }));
    let p = prepare_jsonl_migration(o).await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    p.publish().await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    f.finish().await;
}
#[tokio::test]
async fn awaits_historical_header_validation_before_creating_the_restore() {
    let (f, mut o, format, _) = setup(JsonlCompression::None).await;
    let target = format.clone();
    o.validate_historical_header = Some(Arc::new(move |header| {
        let target = target.clone();
        Box::pin(async move {
            tokio::task::yield_now().await;
            assert_eq!(header["version"], 3);
            assert_eq!(target.created.load(Ordering::SeqCst), 0);
            Ok(())
        })
    }));
    prepare_jsonl_migration(o).await.unwrap();
    assert_eq!(format.created.load(Ordering::SeqCst), 1);
    f.finish().await;
}
#[tokio::test]
async fn rejects_a_malformed_source_header_before_creating_the_restore() {
    for raw in [
        "null\n",
        "[]\n",
        "\"header\"\n",
        "not json\n",
        "{}\n",
        "{\"version\":-0}\n",
        "{\"version\":1.5}\n",
        "{\"version\":9007199254740992}\n",
    ] {
        let (f, o, format, _) = setup(JsonlCompression::None).await;
        tokio::fs::write(&o.source_path, raw).await.unwrap();
        assert!(prepare_jsonl_migration(o).await.is_err(), "{raw}");
        assert_eq!(format.created.load(Ordering::SeqCst), 0);
        f.finish().await;
    }
}
#[tokio::test]
async fn rejects_a_source_filename_and_header_version_mismatch() {
    let (f, o, format, _) = setup(JsonlCompression::None).await;
    tokio::fs::write(&o.source_path, "{\"version\":2}\n")
        .await
        .unwrap();
    fail(
        prepare_jsonl_migration(o).await,
        "filename identifies v3, but its header identifies v2",
    );
    assert_eq!(format.created.load(Ordering::SeqCst), 0);
    f.finish().await;
}
#[tokio::test]
async fn rejects_wrong_target_versions_returned_by_migration() {
    let (f, mut o, _, _) = setup(JsonlCompression::None).await;
    o.format = Arc::new(Format {
        bad_target: true,
        ..Default::default()
    });
    fail(
        prepare_jsonl_migration(o).await,
        "format migration returned v5, expected v4",
    );
    f.finish().await;
}
#[tokio::test]
async fn rejects_noncanonical_source_target_and_directory_paths() {
    let (f, o, _, _) = setup(JsonlCompression::None).await;
    let mut bad = o.clone();
    bad.source_path = bad.source_path.replace("session.v3", "SESSION.v3");
    fail(prepare_jsonl_migration(bad).await, "source path must end");
    let mut bad = o.clone();
    bad.current_path = bad.current_path.replace("session.v4", "session.v04");
    fail(
        prepare_jsonl_migration(bad).await,
        "generation path must end",
    );
    let mut bad = o.clone();
    bad.current_path = "/another/session.v4.jsonl".into();
    fail(
        prepare_jsonl_migration(bad).await,
        "share one Session directory",
    );
    let mut bad = o;
    bad.source_version = 4;
    bad.source_path = bad.current_path.clone();
    fail(
        prepare_jsonl_migration(bad).await,
        "requires a historical source",
    );
    f.finish().await;
}
#[tokio::test]
async fn ignores_byte_torn_and_unparseable_raw_suffixes_but_refuses_a_later_commit() {
    for malformed in ["{torn", "{bad}\n"] {
        let (f, o, _, bytes) = setup(JsonlCompression::None).await;
        append_bytes(&o.source_path, malformed.as_bytes()).await;
        let p = prepare_jsonl_migration(o.clone()).await.unwrap();
        assert_eq!(p.artifact.events.len(), 6);
        assert_eq!(
            tokio::fs::read(&o.source_path).await.unwrap(),
            [bytes, malformed.as_bytes().to_vec()].concat()
        );
        if malformed.ends_with('\n') {
            append_bytes(
                &o.source_path,
                line(&event("turn/end", 6, json!({"turn":2}))).as_bytes(),
            )
            .await;
            fail(prepare_jsonl_migration(o).await, "not valid JSON");
        }
        f.finish().await;
    }
}
#[tokio::test]
async fn refuses_a_complete_zstd_frame_containing_a_torn_jsonl_record() {
    let (f, o, _, _) = setup(JsonlCompression::Zstd).await;
    append_bytes(
        &o.source_path,
        &zstd::compress_zstd_frame("{torn").await.unwrap(),
    )
    .await;
    fail(
        prepare_jsonl_migration(o).await,
        "complete frame contains a torn JSONL record",
    );
    f.finish().await;
}
#[tokio::test]
async fn refuses_an_empty_or_headerless_zstd_source() {
    let (f, o, _, _) = setup(JsonlCompression::Zstd).await;
    tokio::fs::write(&o.source_path, b"").await.unwrap();
    fail(
        prepare_jsonl_migration(o).await,
        "empty or header-less Zstandard session log",
    );
    f.finish().await;
}
#[tokio::test]
async fn requires_a_separate_exactly_one_line_header_frame() {
    let (f, o, _, _) = setup(JsonlCompression::Zstd).await;
    tokio::fs::write(
        &o.source_path,
        zstd::compress_zstd_frame("{\"version\":3}\n{}\n")
            .await
            .unwrap(),
    )
    .await
    .unwrap();
    fail(
        prepare_jsonl_migration(o).await,
        "first frame is not exactly one header line",
    );
    f.finish().await;
}
#[tokio::test]
async fn accepts_an_identical_winner_and_an_appended_winner_without_decoding_the_tail() {
    for compression in [JsonlCompression::None, JsonlCompression::Zstd] {
        for appended in [false, true] {
            let (f, o, _, _) = setup(compression).await;
            let first = prepare_jsonl_migration(o.clone()).await.unwrap();
            first.publish().await.unwrap();
            if appended {
                append_bytes(&o.current_path, b"uninterpreted append tail").await;
            }
            let bytes = tokio::fs::read(&o.current_path).await.unwrap();
            let second = prepare_jsonl_migration(o.clone()).await.unwrap();
            second.publish().await.unwrap();
            assert_eq!(tokio::fs::read(&o.current_path).await.unwrap(), bytes);
            assert_eq!(names(&o.current_path).await.len(), 2);
            f.finish().await;
        }
    }
}
#[tokio::test]
async fn rejects_a_different_or_malformed_winner_without_overwriting_it() {
    for bytes in [b"other\n".as_slice(), b"{\"version\":4}\n"] {
        let (f, o, _, _) = setup(JsonlCompression::None).await;
        let p = prepare_jsonl_migration(o.clone()).await.unwrap();
        tokio::fs::write(&o.current_path, bytes).await.unwrap();
        assert!(has_class::<JsonlGenerationTargetConflictError>(
            fail(p.publish().await, "already exists").as_ref()
        ));
        assert_eq!(tokio::fs::read(&o.current_path).await.unwrap(), bytes);
        assert_eq!(names(&o.current_path).await.len(), 2);
        f.finish().await;
    }
}
#[tokio::test]
async fn rejects_a_symlink_or_non_regular_winner_without_overwriting_it() {
    for symlink in [false, true] {
        let (f, o, _, _) = setup(JsonlCompression::None).await;
        let p = prepare_jsonl_migration(o.clone()).await.unwrap();
        if symlink {
            tokio::fs::symlink(&o.source_path, &o.current_path)
                .await
                .unwrap();
        } else {
            tokio::fs::create_dir(&o.current_path).await.unwrap();
        }
        let e = fail(
            p.publish().await,
            if symlink {
                "symbolic link"
            } else {
                "non-regular file"
            },
        );
        assert!(has_class::<JsonlGenerationTargetConflictError>(e.as_ref()));
        f.finish().await;
    }
}
#[tokio::test]
async fn strict_verification_checks_expected_id_event_count_and_complete_physical_tail() {
    for compression in [JsonlCompression::None, JsonlCompression::Zstd] {
        let (f, o, _, _) = setup(compression).await;
        prepare_jsonl_migration(o.clone())
            .await
            .unwrap()
            .publish()
            .await
            .unwrap();
        fail(
            verify_jsonl_current_generation(&o.current_path, compression, "wrong", 6, None).await,
            "expected \"wrong\"",
        );
        fail(
            verify_jsonl_current_generation(&o.current_path, compression, "generation", 7, None)
                .await,
            "expected 7",
        );
        append_bytes(&o.current_path, b"{torn").await;
        assert!(
            verify_jsonl_current_generation(&o.current_path, compression, "generation", 6, None)
                .await
                .is_err()
        );
        f.finish().await;
    }
}
#[tokio::test]
async fn strict_verification_checks_embedded_assistant_content_usage_and_replay_state() {
    for field in ["content", "usage", "replayState"] {
        let f = Fixture::new(JsonlCompression::None).await;
        let mut rows = log();
        match field {
            "content" => {
                rows[3]["data"]["message"]["content"] = json!([{"type":"text","text":"different"}])
            }
            "usage" => rows[3]["data"]["usage"] = json!({"input":1}),
            _ => rows[3]["data"]["message"]["source"]["replayState"] = json!({"kind":"extra"}),
        }
        let (path, _) = f.store(&meta("generation"), &rows).await;
        fail(
            verify_jsonl_current_generation(&path, JsonlCompression::None, "generation", 6, None)
                .await,
            if field == "replayState" {
                "replay state disagrees"
            } else if field == "usage" {
                "usage disagrees"
            } else {
                "content disagrees"
            },
        );
        f.finish().await;
    }
}
#[tokio::test]
async fn a_bad_event_encoder_cleans_the_stage_and_preserves_the_source() {
    let (f, mut o, _, bytes) = setup(JsonlCompression::Zstd).await;
    o.format = Arc::new(Format {
        encode_failure: true,
        ..Default::default()
    });
    let p = prepare_jsonl_migration(o.clone()).await.unwrap();
    fail(p.publish().await, "event encoder failed");
    assert_eq!(names(&o.source_path).await.len(), 1);
    assert_eq!(tokio::fs::read(o.source_path).await.unwrap(), bytes);
    f.finish().await;
}
async fn failing_stage(operation: IoOperation) {
    let (f, o, _, _) = setup(JsonlCompression::None).await;
    let hook: IoHook = Arc::new(move |r| {
        Box::pin(async move {
            if r.path.contains(".migration.") && r.operation == operation && !r.after {
                return Err(Box::new(std::io::Error::other("injected stage failure")) as BoxError);
            }
            Ok(())
        })
    });
    let runtime = create_jsonl_generation_runtime(JsonlGenerationRuntimeOverrides {
        io_hook: Some(hook),
        ..Default::default()
    });
    let p = runtime.prepare(o.clone()).await.unwrap();
    fail(p.publish().await, "injected stage failure");
    assert_eq!(names(&o.source_path).await.len(), 1);
    f.finish().await;
}
#[tokio::test]
async fn cleans_a_stage_after_open_failure() {
    failing_stage(IoOperation::Open).await;
}
#[tokio::test]
async fn cleans_a_stage_after_write_failure() {
    failing_stage(IoOperation::Write).await;
}
#[tokio::test]
async fn cleans_a_stage_after_sync_failure() {
    failing_stage(IoOperation::Sync).await;
}
#[tokio::test]
async fn cleans_a_stage_after_close_failure() {
    failing_stage(IoOperation::Close).await;
}
#[tokio::test]
async fn aggregates_the_write_failure_and_a_close_failure() {
    let (f, o, _, _) = setup(JsonlCompression::None).await;
    let hook: IoHook = Arc::new(move |r| {
        Box::pin(async move {
            if r.path.contains(".migration.")
                && !r.after
                && matches!(r.operation, IoOperation::Write | IoOperation::Close)
            {
                return Err(
                    Box::new(std::io::Error::other(format!("injected {:?}", r.operation)))
                        as BoxError,
                );
            }
            Ok(())
        })
    });
    let runtime = create_jsonl_generation_runtime(JsonlGenerationRuntimeOverrides {
        io_hook: Some(hook),
        ..Default::default()
    });
    let p = runtime.prepare(o.clone()).await.unwrap();
    let e = fail(p.publish().await, "write and close migration stage");
    let JsonlError::Aggregate { errors, .. } =
        underlying(e.as_ref()).downcast_ref::<JsonlError>().unwrap()
    else {
        panic!("aggregate")
    };
    assert_eq!(errors.len(), 2);
    assert!(errors[0].to_string().contains("Write"));
    assert!(errors[1].to_string().contains("Close"));
    assert_eq!(names(&o.source_path).await.len(), 1);
    f.finish().await;
}
#[tokio::test]
async fn ignores_redundant_committed_temporary_cleanup_failure() {
    let (f, o, _, _) = setup(JsonlCompression::None).await;
    let hook: IoHook = Arc::new(move |r| {
        Box::pin(async move {
            if r.path.contains(".migration.") && !r.after && r.operation == IoOperation::Remove {
                return Err(Box::new(std::io::Error::other("cleanup failed")) as BoxError);
            }
            Ok(())
        })
    });
    let runtime = create_jsonl_generation_runtime(JsonlGenerationRuntimeOverrides {
        io_hook: Some(hook),
        ..Default::default()
    });
    runtime
        .prepare(o.clone())
        .await
        .unwrap()
        .publish()
        .await
        .unwrap();
    assert!(std::path::Path::new(&o.current_path).exists());
    f.finish().await;
}
#[tokio::test]
async fn checks_flush_before_every_file_sync() {
    let (f, o, _, _) = setup(JsonlCompression::None).await;
    let flushed = Arc::new(std::sync::Mutex::new(
        std::collections::HashSet::<String>::new(),
    ));
    let records = flushed.clone();
    let hook: IoHook = Arc::new(move |r| {
        let records = records.clone();
        Box::pin(async move {
            if r.operation == IoOperation::Flush && r.after {
                records.lock().unwrap().insert(r.path.clone());
            }
            if r.operation == IoOperation::Sync && !r.after {
                assert!(
                    records.lock().unwrap().contains(&r.path),
                    "sync without flush: {}",
                    r.path
                );
            }
            Ok(())
        })
    });
    create_jsonl_generation_runtime(JsonlGenerationRuntimeOverrides {
        io_hook: Some(hook),
        ..Default::default()
    })
    .prepare(o)
    .await
    .unwrap()
    .publish()
    .await
    .unwrap();
    f.finish().await;
}
#[tokio::test]
async fn pre_aborted_preparation_performs_no_filesystem_work() {
    let (f, mut o, format, _) = setup(JsonlCompression::None).await;
    let c = hoh_timeout::AbortController::new();
    c.abort();
    o.signal = Some(c.signal());
    let hook: IoHook = Arc::new(|_| Box::pin(async { panic!("unexpected IO") }));
    assert!(
        create_jsonl_generation_runtime(JsonlGenerationRuntimeOverrides {
            io_hook: Some(hook),
            ..Default::default()
        })
        .prepare(o)
        .await
        .is_err()
    );
    assert_eq!(format.created.load(Ordering::SeqCst), 0);
    f.finish().await;
}
#[tokio::test]
async fn cancellation_after_preparation_does_not_interrupt_publication() {
    let (f, mut o, _, _) = setup(JsonlCompression::None).await;
    let c = hoh_timeout::AbortController::new();
    o.signal = Some(c.signal());
    let p = prepare_jsonl_migration(o.clone()).await.unwrap();
    c.abort();
    p.publish().await.unwrap();
    assert!(std::path::Path::new(&o.current_path).exists());
    f.finish().await;
}

struct FailingFormat {
    policy: bool,
    cancellation: Option<(Arc<hoh_timeout::AbortController>, hoh_error::SharedError)>,
}
impl JsonlGenerationFormatAdapter for FailingFormat {
    fn current_version(&self) -> u64 {
        4
    }
    fn create_restore(&self, _: &Value) -> Result<Box<dyn JsonlGenerationRestore>> {
        if let Some((controller, reason)) = &self.cancellation {
            controller.abort_with(reason.clone());
            return Err(Box::new(reason.clone()));
        }
        if self.policy {
            Err(Box::new(
                hoh_session_format::SessionFormatError::unsupported_migration("policy refusal"),
            ))
        } else {
            Err(Box::new(std::io::Error::other(
                "ordinary migration failure",
            )))
        }
    }
    fn encode_header(&self, h: &SessionFormatHeader, cut: u64) -> Result<Value> {
        Format::default().encode_header(h, cut)
    }
    fn encode_event(&self, e: &SessionFormatEvent) -> Result<Value> {
        Format::default().encode_event(e)
    }
}
#[tokio::test]
async fn distinguishes_policy_refusal_from_ordinary_migration_failures() {
    for policy in [false, true] {
        let (f, mut o, _, _) = setup(JsonlCompression::None).await;
        o.format = Arc::new(FailingFormat {
            policy,
            cancellation: None,
        });
        let e = fail(
            prepare_jsonl_migration(o).await,
            if policy {
                "policy refusal"
            } else {
                "ordinary migration failure"
            },
        );
        if policy {
            assert!(e.is::<JsonlGenerationUnsupportedMigrationError>());
            assert_eq!(
                e.downcast_ref::<JsonlGenerationUnsupportedMigrationError>()
                    .unwrap()
                    .from_version,
                3
            );
        } else {
            assert!(e.is::<std::io::Error>());
        }
        f.finish().await;
    }
}
#[tokio::test]
async fn preserves_cancellation_raised_while_creating_a_child_source_decoder() {
    let (f, o, _, _) = setup(JsonlCompression::None).await;
    let controller = Arc::new(hoh_timeout::AbortController::new());
    let reason = hoh_error::SharedError::new(std::io::Error::other("child decoding cancelled"));
    let adapter = FailingFormat {
        policy: false,
        cancellation: Some((controller.clone(), reason.clone())),
    };
    let e = fail(
        read_decoded_jsonl_source(
            &o.source_path,
            3,
            JsonlCompression::None,
            &adapter,
            Some(&controller.signal()),
        )
        .await,
        "child decoding cancelled",
    );
    assert!(
        hoh_error::SharedError::of(e.as_ref())
            .unwrap()
            .ptr_eq(&reason)
    );
    f.finish().await;
}
#[tokio::test]
async fn accepts_an_identical_regular_hardlink_target() {
    let (f, o, _, _) = setup(JsonlCompression::None).await;
    let p = prepare_jsonl_migration(o.clone()).await.unwrap();
    p.publish().await.unwrap();
    let other = format!("{}.identical", o.current_path);
    tokio::fs::hard_link(&o.current_path, &other).await.unwrap();
    tokio::fs::remove_file(&o.current_path).await.unwrap();
    tokio::fs::hard_link(other, &o.current_path).await.unwrap();
    let before = tokio::fs::metadata(&o.current_path).await.unwrap();
    prepare_jsonl_migration(o.clone())
        .await
        .unwrap()
        .publish()
        .await
        .unwrap();
    assert_eq!(
        tokio::fs::metadata(&o.current_path).await.unwrap().ino(),
        before.ino()
    );
    f.finish().await;
}
#[tokio::test]
async fn never_overwrites_a_colliding_exclusive_stage_name() {
    let (f, o, _, _) = setup(JsonlCompression::None).await;
    let directory = std::path::Path::new(&o.current_path).parent().unwrap();
    let occupied = directory.join("session.migration.occupied.jsonl.tmp");
    tokio::fs::write(&occupied, b"do not overwrite")
        .await
        .unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let n = calls.clone();
    let runtime = create_jsonl_generation_runtime(JsonlGenerationRuntimeOverrides {
        random_token: Some(Arc::new(move || {
            if n.fetch_add(1, Ordering::SeqCst) == 0 {
                "occupied".into()
            } else {
                "fresh".into()
            }
        })),
        ..Default::default()
    });
    runtime.prepare(o).await.unwrap().publish().await.unwrap();
    assert_eq!(
        tokio::fs::read(occupied).await.unwrap(),
        b"do not overwrite"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    f.finish().await;
}
#[tokio::test]
async fn retains_a_posix_publication_after_directory_sync_fails() {
    let (f, o, _, _) = setup(JsonlCompression::None).await;
    let directory = std::path::Path::new(&o.current_path)
        .parent()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let hook: IoHook = Arc::new(move |r| {
        let directory = directory.clone();
        Box::pin(async move {
            if r.path == directory && r.operation == IoOperation::Sync && !r.after {
                return Err(Box::new(std::io::Error::other("directory sync failed")) as BoxError);
            }
            Ok(())
        })
    });
    let p = create_jsonl_generation_runtime(JsonlGenerationRuntimeOverrides {
        io_hook: Some(hook),
        ..Default::default()
    })
    .prepare(o.clone())
    .await
    .unwrap();
    fail(p.publish().await, "directory sync failed");
    assert!(std::path::Path::new(&o.current_path).is_file());
    assert_eq!(names(&o.source_path).await.len(), 2);
    f.finish().await;
}
#[tokio::test]
async fn retains_a_committed_generation_when_its_post_publication_stat_fails() {
    let (f, o, _, _) = setup(JsonlCompression::None).await;
    let target = o.current_path.clone();
    let hook: IoHook = Arc::new(move |r| {
        let target = target.clone();
        Box::pin(async move {
            if r.path == target && r.operation == IoOperation::Stat && !r.after {
                return Err(
                    Box::new(std::io::Error::other("post-publication stat failed")) as BoxError,
                );
            }
            Ok(())
        })
    });
    let p = create_jsonl_generation_runtime(JsonlGenerationRuntimeOverrides {
        io_hook: Some(hook),
        ..Default::default()
    })
    .prepare(o.clone())
    .await
    .unwrap();
    fail(p.publish().await, "post-publication stat failed");
    assert!(std::path::Path::new(&o.current_path).is_file());
    assert_eq!(names(&o.source_path).await.len(), 2);
    f.finish().await;
}
#[tokio::test]
async fn preserves_a_publication_failure_when_temporary_cleanup_also_fails() {
    let (f, o, _, _) = setup(JsonlCompression::None).await;
    let hook: IoHook = Arc::new(move |r| {
        Box::pin(async move {
            if !r.after && matches!(r.operation, IoOperation::Link | IoOperation::Remove) {
                return Err(
                    Box::new(std::io::Error::other(if r.operation == IoOperation::Link {
                        "publication failure"
                    } else {
                        "cleanup failure"
                    })) as BoxError,
                );
            }
            Ok(())
        })
    });
    let p = create_jsonl_generation_runtime(JsonlGenerationRuntimeOverrides {
        io_hook: Some(hook),
        ..Default::default()
    })
    .prepare(o.clone())
    .await
    .unwrap();
    let e = fail(p.publish().await, "failed to clean migration temporary");
    let JsonlError::Aggregate { errors, .. } =
        underlying(e.as_ref()).downcast_ref::<JsonlError>().unwrap()
    else {
        panic!("aggregate")
    };
    assert_eq!(errors.len(), 2);
    assert_eq!(errors[0].to_string(), "publication failure");
    assert_eq!(errors[1].to_string(), "cleanup failure");
    assert!(!std::path::Path::new(&o.current_path).exists());
    f.finish().await;
}
#[tokio::test]
async fn does_not_reopen_a_target_after_exclusive_publication() {
    let (f, o, _, _) = setup(JsonlCompression::None).await;
    let target = o.current_path.clone();
    let hook: IoHook = Arc::new(move |r| {
        let target = target.clone();
        Box::pin(async move {
            if r.path == target && r.operation == IoOperation::Open {
                panic!("successful exclusive publication reopened target");
            }
            Ok(())
        })
    });
    create_jsonl_generation_runtime(JsonlGenerationRuntimeOverrides {
        io_hook: Some(hook),
        ..Default::default()
    })
    .prepare(o)
    .await
    .unwrap()
    .publish()
    .await
    .unwrap();
    f.finish().await;
}
#[tokio::test]
async fn bounds_a_bracketed_physical_read_to_one_retry_under_continuous_churn() {
    let (f, o, _, _) = setup(JsonlCompression::None).await;
    let n = Arc::new(AtomicUsize::new(0));
    let count = n.clone();
    let hook: IoHook = Arc::new(move |r| {
        let count = count.clone();
        Box::pin(async move {
            if r.operation == IoOperation::ReadFile && r.after {
                let n = count.fetch_add(1, Ordering::SeqCst);
                append_bytes(
                    &r.path,
                    line(&event("feedback/record", 6 + n as u64, json!({}))).as_bytes(),
                )
                .await;
            }
            Ok(())
        })
    });
    let runtime = create_jsonl_generation_runtime(JsonlGenerationRuntimeOverrides {
        io_hook: Some(hook),
        ..Default::default()
    });
    let snapshot = runtime.read_stable(&o.source_path, None).await.unwrap();
    assert_eq!(n.load(Ordering::SeqCst), 2);
    assert_eq!(snapshot.bytes.len(), snapshot.identity.size as usize);
    assert!(tokio::fs::metadata(&o.source_path).await.unwrap().len() > snapshot.identity.size);
    f.finish().await;
}

#[tokio::test]
async fn returns_migrated_events_while_publication_is_still_waiting_for_verification() {
    let (f, mut o, _, _) = setup(JsonlCompression::None).await;
    let entered = Arc::new(tokio::sync::Notify::new());
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let (notify, wait) = (entered.clone(), gate.clone());
    o.verify_current_file = Arc::new(move |p, c, id, n, prefix, _| {
        let (notify, wait) = (notify.clone(), wait.clone());
        Box::pin(async move {
            notify.notify_one();
            wait.acquire().await.unwrap().forget();
            verify_jsonl_current_generation(&p, c, &id, n, prefix.as_ref()).await
        })
    });
    let p = prepare_jsonl_migration(o.clone()).await.unwrap();
    let publication = p.publish();
    entered.notified().await;
    assert_eq!(p.artifact.events.len(), 6);
    assert!(!std::path::Path::new(&o.current_path).exists());
    gate.add_permits(1);
    publication.await.unwrap();
    f.finish().await;
}
#[tokio::test]
async fn rejects_a_stage_whose_verified_identity_bytes_or_digest_disagree() {
    for wrong_bytes in [false, true] {
        let (f, mut o, _, _) = setup(JsonlCompression::None).await;
        o.verify_current_file = Arc::new(move |p, c, id, n, prefix, _| {
            Box::pin(async move {
                let mut result =
                    verify_jsonl_current_generation(&p, c, &id, n, prefix.as_ref()).await?;
                if wrong_bytes {
                    result.bytes += 1;
                } else {
                    result.digest = "wrong".into();
                }
                Ok(result)
            })
        });
        let p = prepare_jsonl_migration(o.clone()).await.unwrap();
        fail(
            p.publish().await,
            "staged session generation changed during verification",
        );
        assert_eq!(names(&o.source_path).await.len(), 1);
        f.finish().await;
    }
}
#[tokio::test]
async fn prefix_verification_checks_both_length_and_digest_without_interpreting_a_tail() {
    let (f, o, _, _) = setup(JsonlCompression::None).await;
    prepare_jsonl_migration(o.clone())
        .await
        .unwrap()
        .publish()
        .await
        .unwrap();
    let verified =
        verify_jsonl_current_generation(&o.current_path, o.compression, "generation", 6, None)
            .await
            .unwrap();
    let prefix = JsonlExpectedPrefix {
        bytes: verified.bytes,
        digest: verified.digest,
    };
    append_bytes(&o.current_path, b"unparsed suffix").await;
    assert_eq!(
        verify_jsonl_current_generation(
            &o.current_path,
            o.compression,
            "wrong id ignored for proven prefix",
            999,
            Some(&prefix)
        )
        .await
        .unwrap()
        .bytes,
        prefix.bytes
    );
    let bad = JsonlExpectedPrefix {
        bytes: prefix.bytes + 10000,
        digest: prefix.digest.clone(),
    };
    fail(
        verify_jsonl_current_generation(
            &o.current_path,
            o.compression,
            "generation",
            6,
            Some(&bad),
        )
        .await,
        "shorter than the migrated generation",
    );
    let bad = JsonlExpectedPrefix {
        bytes: prefix.bytes,
        digest: "0".repeat(64),
    };
    fail(
        verify_jsonl_current_generation(
            &o.current_path,
            o.compression,
            "generation",
            6,
            Some(&bad),
        )
        .await,
        "do not begin with the migrated generation",
    );
    f.finish().await;
}

struct ThresholdFormat {
    widths: Vec<usize>,
}
impl JsonlGenerationFormatAdapter for ThresholdFormat {
    fn current_version(&self) -> u64 {
        4
    }
    fn create_restore(&self, h: &Value) -> Result<Box<dyn JsonlGenerationRestore>> {
        Format::default().create_restore(h)
    }
    fn encode_header(&self, h: &SessionFormatHeader, c: u64) -> Result<Value> {
        Format::default().encode_header(h, c)
    }
    fn encode_event(&self, e: &SessionFormatEvent) -> Result<Value> {
        Ok(Value::String(
            "x".repeat(self.widths[e["seq"].as_u64().unwrap() as usize]),
        ))
    }
}
#[tokio::test]
async fn batches_exact_threshold_encoded_rows_without_retaining_a_final_partial_write() {
    use sha2::{Digest, Sha256};
    let mib = 1024 * 1024;
    let (f, mut o, _, _) = setup(JsonlCompression::None).await;
    let mut h = to_header_line(&meta("generation"), None).unwrap();
    h["version"] = 3.into();
    let mut source = line(&h);
    for seq in 0..5 {
        source.push_str(&line(&event("feedback/record", seq, json!({}))));
    }
    tokio::fs::write(&o.source_path, source).await.unwrap();
    o.format = Arc::new(ThresholdFormat {
        widths: vec![4 * mib - 3, mib - 3, mib - 3, mib - 3, mib - 3],
    });
    o.verify_current_file = Arc::new(move |p, _, _, n, _, _| {
        Box::pin(async move {
            assert_eq!(n, 5);
            let s = read_stable_jsonl_file(&p, None).await?;
            Ok(JsonlVerifiedGeneration {
                identity: s.identity,
                bytes: s.bytes.len(),
                digest: Sha256::digest(&s.bytes)
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect(),
            })
        })
    });
    let writes = Arc::new(std::sync::Mutex::new(vec![]));
    let count = writes.clone();
    let hook: IoHook = Arc::new(move |r| {
        let count = count.clone();
        Box::pin(async move {
            if r.path.contains(".migration.") && r.operation == IoOperation::Write && !r.after {
                count.lock().unwrap().push(r.bytes.unwrap());
            }
            Ok(())
        })
    });
    create_jsonl_generation_runtime(JsonlGenerationRuntimeOverrides {
        io_hook: Some(hook),
        ..Default::default()
    })
    .prepare(o.clone())
    .await
    .unwrap()
    .publish()
    .await
    .unwrap();
    let writes = writes.lock().unwrap().clone();
    assert_eq!(writes.len(), 3);
    assert_eq!(&writes[1..], &[4 * mib, 4 * mib]);
    assert!(tokio::fs::metadata(o.current_path).await.unwrap().len() > 8 * mib as u64);
    f.finish().await;
}
