mod common;
use common::*;
use hoh_session::{SessionEvent, SessionId, SessionLogOffset, SessionSeedEventState};
use hoh_session_format::SessionFormatRecovery;
use hoh_session_persistence::*;
use hoh_session_persistence_jsonl::{
    JsonlCompression,
    format::*,
    testing::{IoHook, IoOperation},
};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

fn header(id: &str) -> Vec<u8> {
    line(&to_header_line(&meta(id), None).unwrap()).into_bytes()
}
fn scan(rows: &[Value]) -> hoh_session_persistence_jsonl::Result<SessionLogScan> {
    let mut bytes = header("scanner");
    for row in rows {
        bytes.extend(line(row).bytes());
    }
    scan_log(&bytes)
}
fn close_turn() -> Vec<Value> {
    vec![
        event("turn/start", 0, json!({"turn":1})),
        event(
            "turn/end",
            1,
            json!({"turn":1,"reason":{"kind":"completed"}}),
        ),
    ]
}

#[test]
fn names_and_parses_only_canonical_immutable_generations() {
    for (n, name) in [
        (0, "session.jsonl"),
        (1, "session.v1.jsonl"),
        (27, "session.v27.jsonl"),
    ] {
        assert_eq!(
            generation_log_filename(n, JsonlCompression::None).unwrap(),
            name
        );
        assert_eq!(
            parse_generation_log_filename(name, JsonlCompression::None),
            Some(n)
        );
        assert_eq!(
            parse_generation_log_filename(&format!("{name}.zstd"), JsonlCompression::Zstd),
            Some(n)
        );
    }
    for name in [
        "SESSION.v4.jsonl",
        "session.v0.jsonl",
        "session.v04.jsonl",
        "session.v-1.jsonl",
        "session.v1.0.jsonl",
        "session.v9007199254740992.jsonl",
        "session.v4.jsonl.tmp",
        "session.jsonl.zstd",
    ] {
        assert_eq!(
            parse_generation_log_filename(name, JsonlCompression::None),
            None,
            "{name}"
        );
    }
    assert!(generation_log_filename(9_007_199_254_740_992, JsonlCompression::None).is_err());
}
#[test]
fn ignores_retired_header_checks_for_non_object_values() {
    for value in [Value::Null, json!(42), json!([]), json!("x")] {
        assert_no_retired_header_fields(&value).unwrap();
    }
    for key in ["sandboxMode", "approvalPolicy"] {
        fail(
            assert_no_retired_header_fields(&json!({key:null})),
            "retired policy baseline fields",
        );
    }
}
#[test]
fn encode_segment_neutralizes_traversal_separators_and_absolute_paths() {
    for (a, b) in [
        (".", "~002E"),
        ("..", "~002E~002E"),
        ("../x", "..~002Fx"),
        ("/tmp/x", "~002Ftmp~002Fx"),
        ("C:\\x", "C~003A~005Cx"),
        ("a~002Fb", "a~007E002Fb"),
        ("a_b-c.1", "a_b-c.1"),
    ] {
        assert_eq!(encode_segment(a).unwrap(), b);
    }
}
#[test]
fn encode_segment_is_injective_over_utf16_including_lone_surrogates() {
    assert_eq!(encode_segment_utf16(&[0xd800]).unwrap(), "~D800");
    assert_eq!(encode_segment_utf16(&[0xdc00]).unwrap(), "~DC00");
    assert_eq!(encode_segment("🦀").unwrap(), "~D83E~DD80");
    assert_ne!(
        encode_segment_utf16(&[0xd800]).unwrap(),
        encode_segment("~D800").unwrap()
    );
}
#[test]
fn encode_segment_rejects_an_empty_id() {
    fail(encode_segment(""), "cannot encode an empty path segment");
}
#[test]
fn project_key_normalizes_project_paths_into_bounded_readable_names() {
    for (path, key) in [
        (
            "/Users/qyj/work/deepseek-harness",
            "--Users-qyj-work-deepseek-harness--",
        ),
        ("C:\\work\\agent", "--C-work-agent--"),
        ("/开发/~agent", "--~5F00~53D1-~007Eagent--"),
        ("/", "--root--"),
    ] {
        assert_eq!(project_key(path).unwrap(), key);
    }
    assert_eq!(
        project_key("/a/b-c").unwrap(),
        project_key("/a-b/c").unwrap()
    );
    assert_eq!(project_key(&"a".repeat(1000)).unwrap().len(), 255);
}
#[test]
fn round_trips_every_optional_header_field_through_the_header_line() {
    let mut m = meta("all");
    m.cwd = Some("/work".into());
    m.parent_session = Some("parent".into());
    m.is_seeded = true;
    m.origin = Some("subagent".into());
    m.delegation_depth = Some(2);
    m.agent_preset = Some("minimal".into());
    let physical = to_header_line(&m, Some(SessionLogOffset::new(3).unwrap())).unwrap();
    let mut bytes = line(&physical).into_bytes();
    for i in 0..=3 {
        let row = if i == 3 {
            event("session/end-seed", i, json!({"inherited":true}))
        } else {
            json!({"type":"feedback","seq":i,"time":i+1,"data":{}})
        };
        bytes.extend(line(&row).bytes());
    }
    let got = scan_log(&bytes).unwrap();
    assert_eq!(got.meta, m);
    assert_eq!(got.inherited_event_count.get(), 3);
}
#[test]
fn preserves_logical_lineage_independently_of_the_physical_cut() {
    for (seeded, cut) in [(false, 0), (true, 0), (true, 3)] {
        let mut m = meta("lineage");
        m.is_seeded = seeded;
        let h = to_header_line(&m, Some(SessionLogOffset::new(cut).unwrap())).unwrap();
        assert_eq!(h["isSeeded"], seeded);
        assert!(h.get("inheritedEventCount").is_none());
    }
}
#[test]
fn rejects_a_current_physical_header_without_explicit_is_seeded() {
    let mut h = to_header_line(&meta("explicit"), None).unwrap();
    h.as_object_mut().unwrap().shift_remove("isSeeded");
    fail(
        scan_log(line(&h).as_bytes()),
        "first line is not a session header",
    );
}
#[test]
fn requires_logical_lineage_and_the_physical_inherited_cut_to_agree() {
    let mut m = meta("cut");
    fail(
        to_header_line(&m, Some(SessionLogOffset::new(1).unwrap())),
        "unseeded session header inherited event count must be 0",
    );
    m.is_seeded = true;
    fail(
        to_header_line(&m, None),
        "seeded session header requires an inherited event count",
    );
}
#[test]
fn scanner_requires_exactly_one_header_record_and_can_scan_strictly() {
    for h in [b"".as_slice(), b"{}", b"{}\n\n", b"{}\n{}\n"] {
        assert!(SessionLogScanner::new(h, SessionFormatRecovery::Recoverable).is_err());
    }
    let mut s = SessionLogScanner::new(&header("strict"), SessionFormatRecovery::Strict).unwrap();
    fail(s.write(b"null\n"), "invalid committed event");
}
#[test]
fn scanner_expands_packed_source_event_references() {
    let rows = vec![
        event("feedback", 0, json!({})),
        event("feedback", 1, json!({})),
        json!({"type":"user/message","seq":2,"time":3,"data":{"id":"u","role":"user","content":[],"source":{"kind":"user"}},"surfaceOp":"append","sourceEventSeqs":[[0,1]]}),
    ];
    assert_eq!(
        scan(&rows).unwrap().events[2].as_value()["sourceEventSeqs"],
        json!([0, 1])
    );
}
#[test]
fn scanner_rejects_seeded_headers_without_an_inherited_marker() {
    let mut h = meta("seeded");
    h.is_seeded = true;
    let bytes = line(&to_header_line(&h, Some(SessionLogOffset::default())).unwrap());
    fail(
        scan_log(bytes.as_bytes()),
        "seeded Session lacks an inherited end-seed marker",
    );
}
#[test]
fn scanner_rejects_an_inherited_marker_on_an_unseeded_session() {
    fail(
        scan(&[event("session/end-seed", 0, json!({"inherited":true}))]),
        "unseeded Session contains an inherited end-seed marker",
    );
}
#[test]
fn scanner_copies_fragments_and_handles_empty_writes_and_split_utf8() {
    let h = header("split");
    let mut s = SessionLogScanner::new(&h, SessionFormatRecovery::Recoverable).unwrap();
    let row = line(&event("feedback", 0, json!({"text":"猫🦀"})));
    let mut chunk = vec![0u8; 1];
    s.write(&[]).unwrap();
    for b in row.bytes() {
        chunk[0] = b;
        s.write(&chunk).unwrap();
        chunk[0] = 0;
    }
    assert_eq!(s.checkpoint().unwrap().input_bytes, h.len() + row.len());
    let result = s.finish().unwrap();
    assert_eq!(result.events[0].data()["text"], "猫🦀");
    fail(s.write(b"\n"), "finished session log scanner");
}
#[test]
fn scanner_recovers_a_corrupt_suffix_but_refuses_corruption_before_a_later_turn_end() {
    let mut bytes = header("tail");
    for row in close_turn() {
        bytes.extend(line(&row).bytes());
    }
    let committed = bytes.len();
    bytes.extend(b"{bad\n");
    let scan = scan_log(&bytes).unwrap();
    assert_eq!(scan.events.len(), 2);
    assert_eq!(scan.committed_bytes, committed);
    bytes.extend(line(&event("turn/end", 3, json!({"turn":2}))).bytes());
    fail(scan_log(&bytes), "unparsable committed event");
}
#[test]
fn scanner_rejects_a_sequence_gap_on_a_terminal_event_immediately() {
    let mut s = SessionLogScanner::new(&header("gap"), SessionFormatRecovery::Recoverable).unwrap();
    fail(
        s.write(line(&event("turn/end", 9, json!({"turn":1}))).as_bytes()),
        "invalid committed event",
    );
}
#[test]
fn scanner_retains_only_the_valid_dense_prefix_after_a_closed_turn() {
    let mut bytes = header("gap-tail");
    for row in close_turn() {
        bytes.extend(line(&row).bytes());
    }
    let committed = bytes.len();
    bytes.extend(line(&event("feedback", 9, json!({}))).bytes());
    let scan = scan_log(&bytes).unwrap();
    assert_eq!(scan.events.len(), 2);
    assert_eq!(scan.committed_bytes, committed);
}
#[test]
fn scanner_drops_a_final_record_without_its_newline() {
    let mut bytes = header("torn");
    bytes.extend(line(&event("feedback", 0, json!({}))).trim_end().bytes());
    assert!(scan_log(&bytes).unwrap().events.is_empty());
}
#[test]
fn scanner_classifies_scalar_malformed_and_foreign_headers() {
    for value in [json!(42), Value::Null] {
        fail(
            scan_log(line(&value).as_bytes()),
            "first line is not a JSON object",
        );
    }
    for value in [
        json!({"type":"session","version":4,"id":123}),
        json!({"type":"session","version":4,"id":"x","createdAt":0,"isSeeded":false,"delegationDepth":-1}),
    ] {
        fail(
            scan_log(line(&value).as_bytes()),
            "first line is not a session header",
        );
    }
    let error = fail(
        scan_log(b"{\"version\":42,\"id\":123}\n"),
        "session \"123\" uses log format v42",
    );
    assert!(has_class::<SessionFormatUnsupportedError>(error.as_ref()));
}
#[test]
fn scanner_keeps_unknown_required_metadata_vocabulary_neutral() {
    for metadata in [json!({"surfaceOp":"append"}), json!({"sourceEventSeqs":[]})] {
        let mut row = event("external/unknown", 0, json!({}));
        row.as_object_mut()
            .unwrap()
            .extend(metadata.as_object().unwrap().clone());
        assert_eq!(scan(&[row.clone()]).unwrap().events[0].as_value(), &row);
    }
}
#[test]
fn scanner_refuses_malformed_native_message_slots_even_without_a_sealed_turn() {
    for (kind, data, text) in [
        (
            "assistant/message",
            json!({"turn":1,"step":1}),
            "requires a message",
        ),
        (
            "agent/inbox/spliced",
            json!({"inserted":null}),
            "requires message array",
        ),
        (
            "session/title-llm-request",
            json!({"messages":[null]}),
            "requires message objects",
        ),
    ] {
        fail(scan(&[event(kind, 0, data)]), text);
    }
}

#[tokio::test]
async fn create_is_lazy_and_close_erases_a_never_materialized_session() {
    let f = Fixture::new(JsonlCompression::None).await;
    let h = f.backend.create(&meta("lazy"), None).await.unwrap();
    assert!(!std::path::Path::new(&f.path("lazy")).exists());
    let s = f.backend.stat(&"lazy".into(), None).await.unwrap().unwrap();
    assert!(s.location.is_none() && s.size_bytes.is_none() && s.format_status.is_none());
    assert!(s.revision.as_str().starts_with("memory:"));
    assert_eq!(
        h.read(None, None, None).await.unwrap().event_state,
        SessionSeedEventState::Detached
    );
    h.close().await.unwrap();
    assert!(
        f.backend
            .stat(&"lazy".into(), None)
            .await
            .unwrap()
            .is_none()
    );
    f.finish().await;
}
#[tokio::test]
async fn explicit_flush_materializes_an_empty_v4_header() {
    for compression in [JsonlCompression::None, JsonlCompression::Zstd] {
        let f = Fixture::new(compression).await;
        let h = f.backend.create(&meta("empty"), None).await.unwrap();
        h.flush(None).await.unwrap();
        let bytes = tokio::fs::read(f.path("empty")).await.unwrap();
        let decoded = if compression == JsonlCompression::Zstd {
            hoh_session_persistence_jsonl::zstd::decompress_zstd_frame(&bytes)
                .await
                .unwrap()
        } else {
            bytes
        };
        assert_eq!(
            hoh_util_values::js_json_parse(std::str::from_utf8(&decoded).unwrap().trim_end())
                .unwrap()["version"],
            4
        );
        assert!(h.read(None, None, None).await.unwrap().events.is_empty());
        h.close().await.unwrap();
        f.finish().await;
    }
}
#[tokio::test]
async fn appends_and_reads_raw_and_compressed_v4_logs_after_a_fresh_reopen() {
    for compression in [JsonlCompression::None, JsonlCompression::Zstd] {
        let f = Fixture::new(compression).await;
        let h = f.backend.create(&meta("one-turn"), None).await.unwrap();
        h.append(&arcs(&log()), None).await.unwrap();
        assert_eq!(read(h.as_ref()).await, log());
        h.close().await.unwrap();
        let r = f
            .backend
            .open(&"one-turn".into(), SessionAccess::Read, None)
            .await
            .unwrap();
        assert_eq!(read(r.as_ref()).await, log());
        let a = r.read(None, None, None).await.unwrap();
        let b = r.read(None, None, None).await.unwrap();
        assert_eq!(a.event_state, SessionSeedEventState::SharedFrozen);
        assert!(Arc::ptr_eq(&a.events[0], &b.events[0]));
        assert_eq!(
            values(&r.read(Some(2), Some(2), None).await.unwrap().events),
            log()[2..4]
        );
        assert!(r.read(Some(6), None, None).await.unwrap().events.is_empty());
        r.close().await.unwrap();
        f.finish().await;
    }
}
#[tokio::test]
async fn read_only_handles_reject_mutations_and_closed_handles_reject_every_operation() {
    let f = Fixture::new(JsonlCompression::None).await;
    f.store(&meta("reader"), &log()).await;
    let r = f
        .backend
        .open(&"reader".into(), SessionAccess::Read, None)
        .await
        .unwrap();
    assert!(has_class::<SessionReadOnlyError>(
        fail(
            r.append(&arcs(&[event("feedback", 6, json!({}))]), None)
                .await,
            "read handle"
        )
        .as_ref()
    ));
    assert!(has_class::<SessionReadOnlyError>(
        fail(r.flush(None).await, "read handle").as_ref()
    ));
    r.close().await.unwrap();
    r.close().await.unwrap();
    for e in [r.append(&[], None).await, r.flush(None).await] {
        assert!(has_class::<SessionHandleClosedError>(
            fail(e, "closed").as_ref()
        ));
    }
    assert!(has_class::<SessionHandleClosedError>(
        fail(r.read(None, None, None).await, "closed").as_ref()
    ));
    f.finish().await;
}
#[tokio::test]
async fn rejects_noncontiguous_append_without_advancing_the_cursor() {
    let f = Fixture::new(JsonlCompression::None).await;
    let h = f.backend.create(&meta("dense"), None).await.unwrap();
    fail(
        h.append(&arcs(&[event("feedback", 1, json!({}))]), None)
            .await,
        "append seq mismatch",
    );
    h.append(&arcs(&log()), None).await.unwrap();
    assert_eq!(read(h.as_ref()).await, log());
    h.close().await.unwrap();
    f.finish().await;
}
#[tokio::test]
async fn aborts_pre_cancelled_operations_without_materializing() {
    let f = Fixture::new(JsonlCompression::None).await;
    let c = hoh_timeout::AbortController::new();
    c.abort();
    let opts = SessionPersistenceCreateOptions {
        signal: Some(c.signal()),
        ..Default::default()
    };
    assert!(
        f.backend
            .create(&meta("abort-create"), Some(&opts))
            .await
            .is_err()
    );
    let h = f.backend.create(&meta("abort-append"), None).await.unwrap();
    assert!(
        h.append(
            &arcs(&log()),
            Some(&SessionHandleAppendOptions {
                signal: Some(c.signal())
            })
        )
        .await
        .is_err()
    );
    assert!(!std::path::Path::new(&f.path("abort-append")).exists());
    h.close().await.unwrap();
    f.finish().await;
}
#[tokio::test]
async fn stat_and_list_read_headers_without_loading_bodies_and_report_raw_paths() {
    let reads = Arc::new(AtomicUsize::new(0));
    let count = reads.clone();
    let hook: IoHook = Arc::new(move |r| {
        let count = count.clone();
        Box::pin(async move {
            if r.operation == IoOperation::ReadFile && !r.after {
                count.fetch_add(1, Ordering::SeqCst);
            }
            Ok(())
        })
    });
    let f = Fixture::with_hook(JsonlCompression::None, Some(hook)).await;
    let (path, bytes) = f.store(&meta("metadata"), &log()).await;
    let s = f
        .backend
        .stat(&"metadata".into(), None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(s.location.unwrap().path, path);
    assert_eq!(s.size_bytes, Some(bytes.len() as u64));
    assert_eq!(s.format_status, Some(SessionFormatStatus::Current));
    assert_eq!(s.event_count, None);
    assert_eq!(f.backend.list(None).await.unwrap().len(), 1);
    assert_eq!(reads.load(Ordering::SeqCst), 0);
    f.finish().await;
}
#[tokio::test]
async fn corruption_is_typed_and_includes_the_raw_path() {
    for raw in ["42\n", "not json\n", "{\"type\":\"session\"}"] {
        let f = Fixture::new(JsonlCompression::None).await;
        let (path, _) = f.store(&meta("corrupt"), &[]).await;
        tokio::fs::write(&path, raw).await.unwrap();
        let e = fail(
            f.backend
                .open(&"corrupt".into(), SessionAccess::Read, None)
                .await,
            "raw log:",
        );
        assert!(
            has_class::<SessionPersistenceCorruptionError>(e.as_ref()),
            "{e:?}"
        );
        assert!(e.to_string().contains(&path));
        f.finish().await;
    }
}
#[tokio::test]
async fn refuses_a_structurally_foreign_future_header_before_shape_validation_and_skips_it_in_list()
{
    let f = Fixture::new(JsonlCompression::None).await;
    let (path, _) = f
        .store_raw(&json!({"version":42,"id":"future","futureOnly":true}), &[])
        .await;
    for access in [SessionAccess::Read, SessionAccess::Write] {
        let e = fail(
            f.backend.open(&"future".into(), access, None).await,
            "written by a newer harness",
        );
        assert!(has_class::<SessionFormatUnsupportedError>(e.as_ref()));
        assert!(e.to_string().contains(&path));
    }
    assert!(f.backend.list(None).await.unwrap().is_empty());
    f.finish().await;
}
#[tokio::test]
async fn propagates_a_non_format_header_failure_from_stat_and_list() {
    let f = Fixture::new(JsonlCompression::None).await;
    let mut h = serde_json::to_value(meta("retired")).unwrap();
    h["sandboxMode"] = "strict".into();
    f.store_raw(&h, &[]).await;
    fail(
        f.backend.stat(&"retired".into(), None).await,
        "retired policy baseline fields",
    );
    fail(f.backend.list(None).await, "retired policy baseline fields");
    f.finish().await;
}
#[tokio::test]
async fn projects_a_released_v0_header_without_reading_or_mutating_its_body() {
    let f = Fixture::new(JsonlCompression::None).await;
    let (path, bytes) = f
        .store_raw(
            &json!({"version":0,"id":"old","createdAt":1000,"seedLength":0,"agentPreset":"code"}),
            &[json!({"not":"a valid row"})],
        )
        .await;
    let s = f.backend.stat(&"old".into(), None).await.unwrap().unwrap();
    assert_eq!(s.header.version, 4);
    assert_eq!(s.header.agent_preset.as_deref(), Some("ptc"));
    assert_eq!(
        s.format_status,
        Some(SessionFormatStatus::MigrationRequired)
    );
    assert_eq!(f.backend.list(None).await.unwrap().len(), 1);
    assert_eq!(tokio::fs::read(path).await.unwrap(), bytes);
    f.finish().await;
}
#[tokio::test]
async fn service_flush_leaves_read_only_logs_unchanged() {
    let f = Fixture::new(JsonlCompression::None).await;
    let (path, bytes) = f.store(&meta("read-flush"), &log()).await;
    let r = f
        .backend
        .open(&"read-flush".into(), SessionAccess::Read, None)
        .await
        .unwrap();
    f.backend.flush().await.unwrap();
    assert_eq!(tokio::fs::read(path).await.unwrap(), bytes);
    r.close().await.unwrap();
    f.finish().await;
}
#[tokio::test]
async fn a_direct_read_detects_a_stored_prefix_shrinking() {
    let f = Fixture::new(JsonlCompression::None).await;
    let (path, _) = f.store(&meta("shrink"), &log()).await;
    let r = f
        .backend
        .open(&"shrink".into(), SessionAccess::Read, None)
        .await
        .unwrap();
    assert_eq!(read(r.as_ref()).await.len(), 6);
    tokio::fs::write(path, header("shrink")).await.unwrap();
    fail(
        r.read(None, None, None).await,
        "shrank below a previously observed prefix",
    );
    r.close().await.unwrap();
    f.finish().await;
}
#[tokio::test]
async fn writer_repairs_a_torn_raw_suffix_before_appending() {
    let f = Fixture::new(JsonlCompression::None).await;
    let (path, _) = f.store(&meta("repair"), &log()).await;
    append_bytes(&path, b"{\"type\":\"torn").await;
    let w = f
        .backend
        .open(&"repair".into(), SessionAccess::Write, None)
        .await
        .unwrap();
    w.append(&arcs(&[event("turn/start", 6, json!({"turn":2}))]), None)
        .await
        .unwrap();
    assert_eq!(read(w.as_ref()).await.len(), 7);
    assert!(
        !String::from_utf8(tokio::fs::read(path).await.unwrap())
            .unwrap()
            .contains("torn")
    );
    w.close().await.unwrap();
    f.finish().await;
}
#[tokio::test]
async fn an_unchanged_cold_log_parses_once_across_an_observe_then_resume_handoff() {
    let n = Arc::new(AtomicUsize::new(0));
    let count = n.clone();
    let hook: IoHook = Arc::new(move |r| {
        let count = count.clone();
        Box::pin(async move {
            if r.operation == IoOperation::ReadFile && !r.after {
                count.fetch_add(1, Ordering::SeqCst);
            }
            Ok(())
        })
    });
    let f = Fixture::with_hook(JsonlCompression::None, Some(hook)).await;
    f.store(&meta("memo"), &log()).await;
    let r = f
        .backend
        .open(&"memo".into(), SessionAccess::Read, None)
        .await
        .unwrap();
    read(r.as_ref()).await;
    r.close().await.unwrap();
    assert_eq!(n.load(Ordering::SeqCst), 1);
    let w = f
        .backend
        .open(&"memo".into(), SessionAccess::Write, None)
        .await
        .unwrap();
    assert_eq!(read(w.as_ref()).await, log());
    assert_eq!(n.load(Ordering::SeqCst), 1);
    w.append(&arcs(&[event("turn/start", 6, json!({"turn":2}))]), None)
        .await
        .unwrap();
    w.close().await.unwrap();
    let r = f
        .backend
        .open(&"memo".into(), SessionAccess::Read, None)
        .await
        .unwrap();
    assert_eq!(read(r.as_ref()).await.len(), 7);
    assert_eq!(n.load(Ordering::SeqCst), 2);
    r.close().await.unwrap();
    f.finish().await;
}
#[tokio::test]
async fn the_cold_log_memo_evicts_the_oldest_id_outside_the_two_entry_handoff_window() {
    let n = Arc::new(AtomicUsize::new(0));
    let count = n.clone();
    let hook: IoHook = Arc::new(move |r| {
        let count = count.clone();
        Box::pin(async move {
            if r.operation == IoOperation::ReadFile && !r.after {
                count.fetch_add(1, Ordering::SeqCst);
            }
            Ok(())
        })
    });
    let f = Fixture::with_hook(JsonlCompression::None, Some(hook)).await;
    for id in ["a", "b", "c"] {
        f.store(&meta(id), &log()).await;
    }
    for id in ["a", "b", "c", "c", "a"] {
        let r = f
            .backend
            .open(&id.into(), SessionAccess::Read, None)
            .await
            .unwrap();
        read(r.as_ref()).await;
        r.close().await.unwrap();
    }
    assert_eq!(n.load(Ordering::SeqCst), 4);
    f.finish().await;
}
#[tokio::test]
async fn a_foreign_write_misses_the_memo_through_the_revision_guard() {
    let f = Fixture::new(JsonlCompression::None).await;
    f.store(&meta("foreign"), &log()).await;
    let r = f
        .backend
        .open(&"foreign".into(), SessionAccess::Read, None)
        .await
        .unwrap();
    read(r.as_ref()).await;
    let mut config = hoh_session_persistence_jsonl::Config::new(f.root.path().to_string_lossy());
    config.compression = JsonlCompression::None;
    let other =
        hoh_session_persistence_jsonl::JsonlSessionPersistence::new(&f.kernel.root(), config)
            .await
            .unwrap();
    let w = other
        .open(&"foreign".into(), SessionAccess::Write, None)
        .await
        .unwrap();
    w.append(&arcs(&[event("turn/start", 6, json!({"turn":2}))]), None)
        .await
        .unwrap();
    w.close().await.unwrap();
    assert_eq!(read(r.as_ref()).await.len(), 7);
    r.close().await.unwrap();
    other.close().await.unwrap();
    f.finish().await;
}
#[tokio::test]
async fn binds_revisions_to_the_physical_artifact_identity() {
    let a = Fixture::new(JsonlCompression::None).await;
    let b = Fixture::new(JsonlCompression::None).await;
    a.store(&meta("revision"), &log()).await;
    b.store(&meta("revision"), &log()).await;
    let first = a
        .backend
        .stat(&"revision".into(), None)
        .await
        .unwrap()
        .unwrap()
        .revision;
    assert_ne!(
        first,
        b.backend
            .stat(&"revision".into(), None)
            .await
            .unwrap()
            .unwrap()
            .revision
    );
    let mut config = hoh_session_persistence_jsonl::Config::new(a.root.path().to_string_lossy());
    config.compression = JsonlCompression::None;
    let other =
        hoh_session_persistence_jsonl::JsonlSessionPersistence::new(&a.kernel.root(), config)
            .await
            .unwrap();
    assert_eq!(
        first,
        other
            .stat(&"revision".into(), None)
            .await
            .unwrap()
            .unwrap()
            .revision
    );
    other.close().await.unwrap();
    a.finish().await;
    b.finish().await;
}
#[tokio::test]
async fn a_failed_append_rolls_back_partial_bytes_so_retry_has_no_sequence_gap() {
    let armed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = armed.clone();
    let hook: IoHook = Arc::new(move |r| {
        let flag = flag.clone();
        Box::pin(async move {
            if r.operation == IoOperation::Sync
                && !r.after
                && r.path.ends_with(".jsonl")
                && flag.swap(false, Ordering::SeqCst)
            {
                return Err(Box::new(std::io::Error::other("simulated fsync ENOSPC"))
                    as hoh_error::BoxError);
            }
            Ok(())
        })
    });
    let f = Fixture::with_hook(JsonlCompression::None, Some(hook)).await;
    let (path, bytes) = f.store(&meta("rollback"), &log()).await;
    let w = f
        .backend
        .open(&"rollback".into(), SessionAccess::Write, None)
        .await
        .unwrap();
    let rows = arcs(&[event("turn/start", 6, json!({"turn":2}))]);
    armed.store(true, Ordering::SeqCst);
    fail(w.append(&rows, None).await, "ENOSPC");
    assert_eq!(tokio::fs::read(&path).await.unwrap(), bytes);
    w.append(&rows, None).await.unwrap();
    assert_eq!(read(w.as_ref()).await.len(), 7);
    w.close().await.unwrap();
    f.finish().await;
}
#[tokio::test]
async fn reports_both_the_append_failure_and_a_failed_rollback() {
    let armed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = armed.clone();
    let hook: IoHook = Arc::new(move |r| {
        let flag = flag.clone();
        Box::pin(async move {
            if flag.load(Ordering::SeqCst)
                && !r.after
                && r.path.ends_with(".jsonl")
                && matches!(r.operation, IoOperation::Sync | IoOperation::Truncate)
            {
                return Err(
                    Box::new(std::io::Error::other(if r.operation == IoOperation::Sync {
                        "simulated append fsync failure"
                    } else {
                        "simulated rollback failure"
                    })) as hoh_error::BoxError,
                );
            }
            Ok(())
        })
    });
    let f = Fixture::with_hook(JsonlCompression::None, Some(hook)).await;
    f.store(&meta("rollback-fails"), &log()).await;
    let w = f
        .backend
        .open(&"rollback-fails".into(), SessionAccess::Write, None)
        .await
        .unwrap();
    armed.store(true, Ordering::SeqCst);
    let e = fail(
        w.append(&arcs(&[event("turn/start", 6, json!({"turn":2}))]), None)
            .await,
        "failed to roll back append",
    );
    let aggregate = e
        .downcast_ref::<hoh_session_persistence_jsonl::JsonlError>()
        .unwrap();
    let hoh_session_persistence_jsonl::JsonlError::Aggregate { errors, .. } = aggregate else {
        panic!("expected aggregate")
    };
    assert_eq!(errors.len(), 2);
    assert!(errors[0].to_string().contains("append fsync"));
    assert!(errors[1].to_string().contains("rollback failure"));
    armed.store(false, Ordering::SeqCst);
    w.close().await.unwrap();
    f.finish().await;
}
#[tokio::test]
async fn a_stored_open_turn_is_served_as_stored_without_synthetic_closers() {
    let f = Fixture::new(JsonlCompression::None).await;
    let rows = vec![event("turn/start", 0, json!({"turn":1}))];
    f.store(&meta("open-turn"), &rows).await;
    let r = f
        .backend
        .open(&"open-turn".into(), SessionAccess::Read, None)
        .await
        .unwrap();
    assert_eq!(read(r.as_ref()).await, rows);
    r.close().await.unwrap();
    f.finish().await;
}
#[tokio::test]
async fn an_empty_append_batch_does_not_materialize() {
    let f = Fixture::new(JsonlCompression::None).await;
    let h = f.backend.create(&meta("empty-batch"), None).await.unwrap();
    h.append(&[], None).await.unwrap();
    assert!(!std::path::Path::new(&f.path("empty-batch")).exists());
    h.append(&arcs(&log()), None).await.unwrap();
    h.close().await.unwrap();
    f.finish().await;
}
#[tokio::test]
async fn a_read_handle_over_an_erased_pending_session_fails_loudly() {
    let f = Fixture::new(JsonlCompression::None).await;
    let w = f.backend.create(&meta("erased"), None).await.unwrap();
    let r = f
        .backend
        .open(&"erased".into(), SessionAccess::Read, None)
        .await
        .unwrap();
    assert!(read(r.as_ref()).await.is_empty());
    w.close().await.unwrap();
    fail(r.read(None, None, None).await, "not found");
    r.close().await.unwrap();
    f.finish().await;
}
#[tokio::test]
async fn lists_a_pending_id_once_when_an_artifact_appears_beneath_it() {
    let f = Fixture::new(JsonlCompression::None).await;
    let w = f.backend.create(&meta("shadowed"), None).await.unwrap();
    f.store(&meta("shadowed"), &log()).await;
    let entries = f.backend.list(None).await.unwrap();
    assert_eq!(entries.len(), 1);
    assert!(entries[0].size_bytes.is_some());
    w.close().await.unwrap();
    f.finish().await;
}
#[test]
fn event_lines_keep_one_event_per_line_with_nested_compact_stream_records() {
    let plain = arcs(&log())
        .iter()
        .map(|e| (**e).clone())
        .collect::<Vec<_>>();
    let text = event_lines(&plain).unwrap();
    assert_eq!(text.lines().count(), 6);
    let mut bytes = header("rows");
    bytes.extend(text.bytes());
    bytes.push(b'\n');
    assert_eq!(scan_log(&bytes).unwrap().events, plain);
}
#[test]
fn scanner_refuses_removed_top_level_packed_rows_before_a_committed_boundary() {
    for row in [
        json!({"type":"text-chunks","seq0":1,"time0":2,"data":{"dt":[1],"texts":["a","b"]}}),
        json!({"type":"text-chunks","seq0":0,"time0":1,"data":{"dt":[],"texts":["a","b"]}}),
    ] {
        fail(
            scan(&[row, event("turn/end", 2, json!({"turn":1}))]),
            "lacks required field seq",
        );
    }
}
#[test]
fn scanner_drops_a_whole_removed_packed_row_after_the_last_boundary() {
    let first = event("turn/start", 0, json!({"turn":1}));
    let got = scan(&[
        first.clone(),
        json!({"type":"text-chunks","seq0":2,"time0":2,"data":{}}),
    ])
    .unwrap();
    assert_eq!(got.events, vec![SessionEvent::from_value(first)]);
}
#[tokio::test]
async fn list_discovers_sessions_across_multiple_project_directories() {
    let f = Fixture::new(JsonlCompression::None).await;
    for (id, cwd) in [("a", Some("/projA")), ("b", Some("/projB")), ("c", None)] {
        let mut m = meta(id);
        m.cwd = cwd.map(str::to_string);
        f.store(&m, &log()).await;
    }
    let mut ids = f
        .backend
        .list(None)
        .await
        .unwrap()
        .into_iter()
        .map(|s| s.header.id)
        .collect::<Vec<_>>();
    ids.sort();
    assert_eq!(ids, vec!["a", "b", "c"]);
    f.finish().await;
}
#[tokio::test]
async fn groups_sessions_whose_cwd_paths_normalize_to_the_same_project_directory() {
    let f = Fixture::new(JsonlCompression::None).await;
    for (id, cwd) in [("first", "/a/b-c"), ("second", "/a-b/c")] {
        let mut m = meta(id);
        m.cwd = Some(cwd.into());
        f.store(&m, &log()).await;
    }
    assert_eq!(f.backend.list(None).await.unwrap().len(), 2);
    f.finish().await;
}
#[tokio::test]
async fn list_on_an_empty_root_returns_nothing() {
    let f = Fixture::new(JsonlCompression::None).await;
    assert!(f.backend.list(None).await.unwrap().is_empty());
    f.finish().await;
}
#[tokio::test]
async fn list_re_reads_only_headers_whose_file_revision_changed() {
    let n = Arc::new(AtomicUsize::new(0));
    let count = n.clone();
    let hook: IoHook = Arc::new(move |r| {
        let count = count.clone();
        Box::pin(async move {
            if r.operation == IoOperation::Read && !r.after {
                count.fetch_add(1, Ordering::SeqCst);
            }
            Ok(())
        })
    });
    let f = Fixture::with_hook(JsonlCompression::None, Some(hook)).await;
    for id in ["kept", "changed"] {
        f.store(&meta(id), &log()).await;
    }
    let mut first = f.backend.list(None).await.unwrap();
    assert_eq!(n.load(Ordering::SeqCst), 2);
    first[0].header.created_at = 0;
    assert!(
        f.backend
            .list(None)
            .await
            .unwrap()
            .iter()
            .all(|s| s.header.created_at == 1000)
    );
    assert_eq!(n.load(Ordering::SeqCst), 2);
    let mut h = meta("changed");
    h.created_at = 1001;
    f.store(&h, &log()).await;
    let next = f.backend.list(None).await.unwrap();
    assert_eq!(
        next.iter()
            .find(|s| s.header.id.as_str() == "changed")
            .unwrap()
            .header
            .created_at,
        1001
    );
    assert_eq!(n.load(Ordering::SeqCst), 3);
    f.finish().await;
}
#[tokio::test]
async fn keeps_the_transcript_in_an_extensible_session_owned_directory() {
    let f = Fixture::new(JsonlCompression::None).await;
    let (path, _) = f.store(&meta("owned"), &log()).await;
    tokio::fs::write(
        std::path::Path::new(&path)
            .parent()
            .unwrap()
            .join("metadata.json"),
        b"{}\n",
    )
    .await
    .unwrap();
    assert_eq!(f.backend.list(None).await.unwrap().len(), 1);
    let r = f
        .backend
        .open(&"owned".into(), SessionAccess::Read, None)
        .await
        .unwrap();
    assert_eq!(read(r.as_ref()).await, log());
    r.close().await.unwrap();
    f.finish().await;
}
#[tokio::test]
async fn rejects_the_obsolete_flat_file_layout_instead_of_ignoring_stored_sessions() {
    for suffix in ["jsonl", "jsonl.zstd"] {
        let f = Fixture::new(JsonlCompression::None).await;
        let dir = project_dir(&f.root.path().to_string_lossy(), None).unwrap();
        tokio::fs::create_dir_all(&dir).await.unwrap();
        tokio::fs::write(format!("{dir}/flat.{suffix}"), b"legacy")
            .await
            .unwrap();
        fail(
            f.backend
                .open(&"flat".into(), SessionAccess::Read, None)
                .await,
            "unsupported flat-file layout",
        );
        fail(f.backend.list(None).await, "unsupported flat-file layout");
        f.finish().await;
    }
}
#[tokio::test]
async fn list_skips_empty_and_non_header_session_logs() {
    let f = Fixture::new(JsonlCompression::None).await;
    f.store(&meta("real"), &log()).await;
    for (id, raw) in [
        ("empty", ""),
        ("notheader", "{\"type\":\"turn/start\"}\n"),
        ("badjson", "not json\n"),
    ] {
        let (path, _) = f.store(&meta(id), &[]).await;
        tokio::fs::write(path, raw).await.unwrap();
    }
    let entries = f.backend.list(None).await.unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].header.id.as_str(), "real");
    f.finish().await;
}
#[tokio::test]
async fn list_reads_a_header_line_longer_than_the_eight_kib_read_chunk() {
    let f = Fixture::new(JsonlCompression::None).await;
    let mut m = meta("big");
    m.agent_preset = Some("x".repeat(9000));
    f.store(&m, &[]).await;
    assert_eq!(
        f.backend.list(None).await.unwrap()[0]
            .header
            .agent_preset
            .as_ref()
            .unwrap()
            .len(),
        9000
    );
    f.finish().await;
}
#[tokio::test]
async fn rejects_a_mismatched_header_id_without_serving_either_log() {
    let f = Fixture::new(JsonlCompression::None).await;
    let (path, _) = f.store(&meta("a"), &log()).await;
    let bytes =
        tokio::fs::read_to_string(&path)
            .await
            .unwrap()
            .replacen("\"id\":\"a\"", "\"id\":\"b\"", 1);
    tokio::fs::write(&path, &bytes).await.unwrap();
    fail(
        f.backend.open(&"a".into(), SessionAccess::Read, None).await,
        "requested id \"a\" does not match header id \"b\"",
    );
    assert_eq!(tokio::fs::read_to_string(path).await.unwrap(), bytes);
    f.finish().await;
}
#[tokio::test]
async fn list_rejects_a_header_whose_cwd_does_not_identify_its_physical_log() {
    let f = Fixture::new(JsonlCompression::None).await;
    let (path, _) = f.store(&meta("misplaced"), &[]).await;
    let mut h = meta("misplaced");
    h.cwd = Some("/elsewhere".into());
    tokio::fs::write(path, line(&to_header_line(&h, None).unwrap()))
        .await
        .unwrap();
    fail(f.backend.list(None).await, "and cwd identify");
    f.finish().await;
}
#[tokio::test]
async fn open_and_list_reject_an_id_materialized_in_multiple_project_directories() {
    let f = Fixture::new(JsonlCompression::None).await;
    for cwd in ["/a", "/b"] {
        let mut m = meta("duplicate");
        m.cwd = Some(cwd.into());
        f.store(&m, &log()).await;
    }
    fail(
        f.backend
            .open(&"duplicate".into(), SessionAccess::Read, None)
            .await,
        "appears in multiple project directories",
    );
    fail(
        f.backend.list(None).await,
        "appears in multiple project directories",
    );
    f.finish().await;
}
#[tokio::test]
async fn create_rejects_an_id_already_on_disk_under_a_different_project_directory() {
    let f = Fixture::new(JsonlCompression::None).await;
    let mut m = meta("dup");
    m.cwd = Some("/a".into());
    f.store(&m, &log()).await;
    m.cwd = Some("/b".into());
    assert!(has_class::<SessionAlreadyExistsError>(
        fail(f.backend.create(&m, None).await, "already exists").as_ref()
    ));
    f.finish().await;
}
#[tokio::test]
async fn list_returns_nothing_when_the_root_directory_does_not_exist() {
    let f = Fixture::new(JsonlCompression::None).await;
    let config =
        hoh_session_persistence_jsonl::Config::new(f.root.path().join("absent").to_string_lossy());
    let backend =
        hoh_session_persistence_jsonl::JsonlSessionPersistence::new(&f.kernel.root(), config)
            .await
            .unwrap();
    assert!(backend.list(None).await.unwrap().is_empty());
    assert!(!f.root.path().join("absent").exists());
    backend.close().await.unwrap();
    f.finish().await;
}
#[tokio::test]
async fn plugin_load_rejects_an_existing_root_that_is_not_a_directory() {
    let f = Fixture::new(JsonlCompression::None).await;
    let path = f.root.path().join("file");
    tokio::fs::write(&path, b"x").await.unwrap();
    fail(
        hoh_session_persistence_jsonl::JsonlSessionPersistence::new(
            &f.kernel.root(),
            hoh_session_persistence_jsonl::Config::new(path.to_string_lossy()),
        )
        .await,
        "ENOTDIR",
    );
    f.finish().await;
}
#[tokio::test]
async fn list_surfaces_a_root_that_becomes_unusable_after_load() {
    let f = Fixture::new(JsonlCompression::None).await;
    tokio::fs::remove_dir_all(f.root.path()).await.unwrap();
    tokio::fs::write(f.root.path(), b"not a directory")
        .await
        .unwrap();
    fail(f.backend.list(None).await, "ENOTDIR");
    f.finish().await;
}
#[tokio::test]
async fn backend_teardown_closes_handles_left_open() {
    let f = Fixture::new(JsonlCompression::None).await;
    let w = f.backend.create(&meta("teardown"), None).await.unwrap();
    w.append(&arcs(&log()), None).await.unwrap();
    f.backend.close().await.unwrap();
    fail(
        w.append(&arcs(&[event("turn/start", 6, json!({"turn":2}))]), None)
            .await,
        "closed handle",
    );
    f.finish().await;
}
#[tokio::test]
async fn accepts_well_formed_json_values_in_opaque_message_metadata() {
    let f = Fixture::new(JsonlCompression::None).await;
    let row = json!({"type":"user/message","seq":0,"time":1,"surfaceOp":"append","data":{"id":"json","role":"user","content":[{"type":"text","text":"x"}],"source":{"kind":"user"},"extra":{"a":null,"b":true,"c":[1,2,{"d":"nested"}]}}});
    let w = f.backend.create(&meta("json-ok"), None).await.unwrap();
    w.append(&arcs(std::slice::from_ref(&row)), None)
        .await
        .unwrap();
    assert_eq!(read(w.as_ref()).await, vec![row]);
    w.close().await.unwrap();
    f.finish().await;
}

#[tokio::test(start_paused = true)]
async fn shares_the_configured_listing_limit_between_simultaneous_list_calls() {
    let active = Arc::new(AtomicUsize::new(0));
    let maximum = Arc::new(AtomicUsize::new(0));
    let enabled = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (a, m, e) = (active.clone(), maximum.clone(), enabled.clone());
    let hook: IoHook = Arc::new(move |r| {
        let (a, m, e) = (a.clone(), m.clone(), e.clone());
        Box::pin(async move {
            if !r.after
                && e.load(Ordering::SeqCst)
                && matches!(
                    r.operation,
                    IoOperation::ReadDir | IoOperation::Read | IoOperation::Stat
                )
            {
                let n = a.fetch_add(1, Ordering::SeqCst) + 1;
                m.fetch_max(n, Ordering::SeqCst);
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                a.fetch_sub(1, Ordering::SeqCst);
            }
            Ok(())
        })
    });
    let root = tempfile::tempdir().unwrap();
    let kernel = hoh_kernel::Kernel::start();
    let mut config = hoh_session_persistence_jsonl::Config::new(root.path().to_string_lossy());
    config.compression = JsonlCompression::None;
    config.list_concurrency = 2;
    let backend = hoh_session_persistence_jsonl::JsonlSessionPersistence::with_io_hook(
        &kernel.root(),
        config,
        Some(hook),
    )
    .await
    .unwrap();
    let f = Fixture {
        kernel,
        root,
        backend,
    };
    for id in ["a", "b", "c", "d", "e", "f"] {
        f.store(&meta(id), &log()).await;
    }
    enabled.store(true, Ordering::SeqCst);
    let (a, b) = tokio::join!(f.backend.list(None), f.backend.list(None));
    assert_eq!(a.unwrap().len(), 6);
    assert_eq!(b.unwrap().len(), 6);
    assert_eq!(maximum.load(Ordering::SeqCst), 2);
    assert_eq!(active.load(Ordering::SeqCst), 0);
    f.finish().await;
}

#[tokio::test]
async fn rejects_pre_aborted_operations_with_the_exact_cancellation_reason() {
    let f = Fixture::new(JsonlCompression::None).await;
    let id = "pre-aborted".into();
    f.store(&meta("pre-aborted"), &log()).await;
    let reason =
        hoh_error::SharedError::new(std::io::Error::other("persistence operation cancelled"));
    let c = hoh_timeout::AbortController::new();
    c.abort_with(reason.clone());
    let signal = c.signal();
    let errors = vec![
        fail(
            f.backend
                .create(
                    &meta("aborted-create"),
                    Some(&SessionPersistenceCreateOptions {
                        signal: Some(signal.clone()),
                        ..Default::default()
                    }),
                )
                .await,
            "persistence operation cancelled",
        ),
        fail(
            f.backend
                .open(
                    &id,
                    SessionAccess::Read,
                    Some(&SessionPersistenceOpenOptions {
                        signal: Some(signal.clone()),
                    }),
                )
                .await,
            "persistence operation cancelled",
        ),
        fail(
            f.backend
                .open(
                    &id,
                    SessionAccess::Write,
                    Some(&SessionPersistenceOpenOptions {
                        signal: Some(signal.clone()),
                    }),
                )
                .await,
            "persistence operation cancelled",
        ),
        fail(
            f.backend
                .stat(
                    &id,
                    Some(&SessionPersistenceStatOptions {
                        signal: Some(signal.clone()),
                    }),
                )
                .await,
            "persistence operation cancelled",
        ),
        fail(
            f.backend
                .list(Some(&SessionPersistenceListOptions {
                    signal: Some(signal.clone()),
                }))
                .await,
            "persistence operation cancelled",
        ),
    ];
    for e in errors {
        assert!(
            hoh_error::SharedError::of(e.as_ref())
                .unwrap()
                .ptr_eq(&reason)
        );
    }
    let w = f
        .backend
        .open(&id, SessionAccess::Write, None)
        .await
        .unwrap();
    let errors = vec![
        fail(
            w.read(
                None,
                None,
                Some(&SessionHandleReadOptions {
                    signal: Some(signal.clone()),
                }),
            )
            .await,
            "persistence operation cancelled",
        ),
        fail(
            w.append(
                &arcs(&[event("turn/start", 6, json!({"turn":2}))]),
                Some(&SessionHandleAppendOptions {
                    signal: Some(signal.clone()),
                }),
            )
            .await,
            "persistence operation cancelled",
        ),
        fail(
            w.flush(Some(&SessionHandleFlushOptions {
                signal: Some(signal),
            }))
            .await,
            "persistence operation cancelled",
        ),
    ];
    for e in errors {
        assert!(
            hoh_error::SharedError::of(e.as_ref())
                .unwrap()
                .ptr_eq(&reason)
        );
    }
    assert_eq!(read(w.as_ref()).await, log());
    w.close().await.unwrap();
    f.finish().await;
}

#[test]
fn rejects_invalid_creation_timestamps_delegation_depths_and_presets_including_negative_zero() {
    for key in ["createdAt", "delegationDepth"] {
        for value in [
            json!(-1),
            json!(1.5),
            json!("1"),
            Value::Null,
            json!(9_007_199_254_740_992u64),
        ] {
            let mut physical = to_header_line(&meta("bad"), None).unwrap();
            physical[key] = value;
            fail(
                scan_log(line(&physical).as_bytes()),
                "first line is not a session header",
            );
        }
        let physical = line(&to_header_line(&meta("bad"), None).unwrap()).replace(
            &format!("\"{key}\":{}", if key == "createdAt" { 1000 } else { 0 }),
            &format!("\"{key}\":-0"),
        );
        fail(
            scan_log(physical.as_bytes()),
            "first line is not a session header",
        );
    }
    let mut physical = to_header_line(&meta("bad"), None).unwrap();
    physical["agentPreset"] = 1.into();
    fail(
        scan_log(line(&physical).as_bytes()),
        "first line is not a session header",
    );
}
#[test]
fn scanner_handles_header_only_empty_corrupt_and_non_session_logs() {
    let bytes = header("only");
    let scan = scan_log(&bytes).unwrap();
    assert!(scan.events.is_empty());
    assert_eq!(scan.committed_bytes, bytes.len());
    for bytes in [b"".as_slice(), b"no newline"] {
        fail(scan_log(bytes), "empty or header-less session log");
    }
    fail(scan_log(b"{bad}\n"), "header line is not valid JSON");
    fail(
        scan_log(b"{\"type\":\"turn/start\"}\n"),
        "first line is not a session header",
    );
}
#[test]
fn scanner_rejects_invalid_committed_rows_before_a_later_boundary() {
    for invalid in ["{bad}", "null", "{\"type\":\"assistant/message\"}"] {
        let bytes = [
            header("bad-row"),
            format!("{invalid}\n").into_bytes(),
            line(&event("turn/end", 1, json!({"turn":1}))).into_bytes(),
        ]
        .concat();
        let e = fail(scan_log(&bytes), "committed event");
        assert!(e.to_string().contains("unparsable") || e.to_string().contains("invalid"));
    }
}
#[test]
fn scanner_refuses_malformed_native_tool_results_before_recovery_even_when_sealed() {
    for sealed in [false, true] {
        let mut rows = vec![event(
            "tool/result",
            0,
            json!({"turn":1,"step":1,"message":[]}),
        )];
        if sealed {
            rows.push(event("turn/end", 1, json!({"turn":1})));
        }
        fail(scan(&rows), "message must be an object");
    }
}
#[tokio::test]
async fn resolves_a_relative_custom_root_before_storing_a_session() {
    let f = Fixture::new(JsonlCompression::None).await;
    let cwd = std::env::current_dir().unwrap();
    let relative = hoh_util_workspace_path::node_path::relative(
        &cwd.to_string_lossy(),
        &cwd.to_string_lossy(),
        &f.root.path().to_string_lossy(),
    );
    let mut config = hoh_session_persistence_jsonl::Config::new(relative);
    config.compression = JsonlCompression::None;
    let backend =
        hoh_session_persistence_jsonl::JsonlSessionPersistence::new(&f.kernel.root(), config)
            .await
            .unwrap();
    assert_eq!(backend.root(), f.root.path().to_str().unwrap());
    let w = backend.create(&meta("relative"), None).await.unwrap();
    w.append(&arcs(&log()), None).await.unwrap();
    w.close().await.unwrap();
    assert!(std::path::Path::new(&f.path("relative")).is_file());
    backend.close().await.unwrap();
    f.finish().await;
}
#[tokio::test]
async fn classifies_an_unparsable_future_generation_header_as_corruption() {
    let f = Fixture::new(JsonlCompression::None).await;
    let (path, _) = f
        .store_raw(&json!({"version":42,"id":"future-bad"}), &[])
        .await;
    tokio::fs::write(&path, b"{not-json}\n").await.unwrap();
    assert!(has_class::<SessionPersistenceCorruptionError>(
        fail(
            f.backend
                .open(&"future-bad".into(), SessionAccess::Read, None)
                .await,
            "header line is not valid JSON"
        )
        .as_ref()
    ));
    f.finish().await;
}
#[tokio::test]
async fn names_a_foreign_version_header_by_its_stringified_non_string_id() {
    let f = Fixture::new(JsonlCompression::None).await;
    let (path, _) = f
        .store_raw(&json!({"version":42,"id":"numeric-id"}), &[])
        .await;
    tokio::fs::write(&path, b"{\"type\":\"session\",\"version\":42,\"id\":123}\n")
        .await
        .unwrap();
    fail(
        f.backend
            .open(&"numeric-id".into(), SessionAccess::Read, None)
            .await,
        "session \"123\" uses log format v42",
    );
    f.finish().await;
}
#[tokio::test]
async fn treats_a_historical_generation_as_an_existing_id_at_create() {
    let f = Fixture::new(JsonlCompression::None).await;
    f.store_raw(
        &json!({"version":3,"id":"historical","createdAt":1000,"isSeeded":false}),
        &[],
    )
    .await;
    assert!(has_class::<SessionAlreadyExistsError>(
        fail(
            f.backend.create(&meta("historical"), None).await,
            "already exists"
        )
        .as_ref()
    ));
    f.finish().await;
}
#[tokio::test]
async fn reports_format_metadata_from_the_highest_physical_generation_without_body_reads() {
    let f = Fixture::new(JsonlCompression::None).await;
    for version in [0, 1, 2, 3, 4] {
        let mut h = serde_json::to_value(meta("highest")).unwrap();
        h["version"] = version.into();
        if version < 2 {
            h.as_object_mut().unwrap().shift_remove("isSeeded");
        }
        f.store_raw(&h, &[json!({"bad":"row"})]).await;
    }
    let s = f
        .backend
        .stat(&"highest".into(), None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(s.format_status, Some(SessionFormatStatus::Current));
    assert!(s.location.unwrap().path.ends_with("session.v4.jsonl"));
    assert_eq!(f.backend.list(None).await.unwrap().len(), 1);
    f.finish().await;
}
#[tokio::test]
async fn refuses_a_selected_generation_whose_filename_and_header_versions_disagree() {
    let f = Fixture::new(JsonlCompression::None).await;
    let (path, _) = f.store(&meta("version-mismatch"), &[]).await;
    let mut h = to_header_line(&meta("version-mismatch"), None).unwrap();
    h["version"] = 3.into();
    tokio::fs::write(path, line(&h)).await.unwrap();
    fail(
        f.backend.stat(&"version-mismatch".into(), None).await,
        "filename identifies v4, but its header identifies v3",
    );
    f.finish().await;
}
#[tokio::test]
async fn selects_a_retained_future_generation_above_historical_bytes() {
    let f = Fixture::new(JsonlCompression::None).await;
    for version in [3, 42] {
        let mut h = serde_json::to_value(meta("future-selected")).unwrap();
        h["version"] = version.into();
        f.store_raw(&h, &[]).await;
    }
    let e = fail(
        f.backend
            .open(&"future-selected".into(), SessionAccess::Read, None)
            .await,
        "uses log format v42",
    );
    assert!(e.to_string().contains("session.v42.jsonl"));
    f.finish().await;
}
#[tokio::test]
async fn attaches_the_current_path_when_a_direct_handle_read_finds_a_foreign_header() {
    let f = Fixture::new(JsonlCompression::None).await;
    let (path, _) = f.store(&meta("direct-foreign"), &log()).await;
    let r = f
        .backend
        .open(&"direct-foreign".into(), SessionAccess::Read, None)
        .await
        .unwrap();
    let mut h = to_header_line(&meta("direct-foreign"), None).unwrap();
    h["version"] = 7.into();
    tokio::fs::write(&path, line(&h)).await.unwrap();
    let e = fail(r.read(None, None, None).await, "uses log format v7");
    assert!(has_class::<SessionFormatUnsupportedError>(e.as_ref()));
    assert!(e.to_string().contains(&path));
    r.close().await.unwrap();
    f.finish().await;
}
#[tokio::test]
async fn path_traversal_session_ids_are_neutralized_without_escaping_the_root() {
    let f = Fixture::new(JsonlCompression::None).await;
    let id = "../../etc/pwn";
    let w = f.backend.create(&meta(id), None).await.unwrap();
    w.append(&arcs(&log()), None).await.unwrap();
    w.close().await.unwrap();
    let path = f.path(id);
    assert!(std::path::Path::new(&path).starts_with(f.root.path()));
    assert!(std::path::Path::new(&path).is_file());
    assert!(path.contains("..~002F..~002Fetc~002Fpwn"));
    f.finish().await;
}
#[tokio::test]
async fn accepts_an_alternate_project_path_only_when_it_identifies_the_same_physical_log() {
    let f = Fixture::new(JsonlCompression::None).await;
    let mut m = meta("physical-alias");
    m.cwd = Some("/stored".into());
    let (path, _) = f.store(&m, &log()).await;
    let root = f.root.path().to_string_lossy();
    tokio::fs::symlink(
        project_dir(&root, m.cwd.as_deref()).unwrap(),
        project_dir(&root, Some("/alias")).unwrap(),
    )
    .await
    .unwrap();
    m.cwd = Some("/alias".into());
    let mut bytes = line(&to_header_line(&m, None).unwrap());
    for row in log() {
        bytes.push_str(&line(&row));
    }
    tokio::fs::write(path, bytes).await.unwrap();
    let r = f
        .backend
        .open(&m.id, SessionAccess::Read, None)
        .await
        .unwrap();
    assert_eq!(r.header().cwd.as_deref(), Some("/alias"));
    r.close().await.unwrap();
    assert_eq!(f.backend.list(None).await.unwrap().len(), 1);
    f.finish().await;
}
#[tokio::test]
async fn list_rejects_a_header_whose_id_cannot_name_a_storage_path() {
    let f = Fixture::new(JsonlCompression::None).await;
    let (path, _) = f.store(&meta("invalid-id"), &[]).await;
    let mut h = to_header_line(&meta("invalid-id"), None).unwrap();
    h["id"] = "".into();
    tokio::fs::write(path, line(&h)).await.unwrap();
    fail(
        f.backend.list(None).await,
        "header id cannot name a storage path",
    );
    f.finish().await;
}
#[tokio::test]
async fn a_project_directory_storage_fault_surfaces_at_the_first_materializing_write() {
    let f = Fixture::new(JsonlCompression::None).await;
    let mut m = meta("exists-fault");
    m.cwd = Some("/x".into());
    tokio::fs::write(
        project_dir(&f.root.path().to_string_lossy(), m.cwd.as_deref()).unwrap(),
        b"x",
    )
    .await
    .unwrap();
    let w = f.backend.create(&m, None).await.unwrap();
    let e = match w.append(&arcs(&log()), None).await {
        Ok(()) => panic!("expected filesystem fault"),
        Err(e) => e,
    };
    assert!(e.to_string().contains("EEXIST") || e.to_string().contains("ENOTDIR"));
    w.close().await.unwrap();
    f.finish().await;
}
#[tokio::test]
async fn validates_list_concurrency_as_a_positive_integer_that_defaults_to_three() {
    let f = Fixture::new(JsonlCompression::None).await;
    let mut config = hoh_session_persistence_jsonl::Config::new(f.root.path().to_string_lossy());
    assert_eq!(config.list_concurrency, 3);
    config.list_concurrency = 0;
    fail(
        hoh_session_persistence_jsonl::JsonlSessionPersistence::new(&f.kernel.root(), config).await,
        "listConcurrency must be a positive integer",
    );
    assert!(
        serde_json::from_value::<hoh_session_persistence_jsonl::Config>(
            json!({"root":"x","listConcurrency":1.5})
        )
        .is_err()
    );
    f.finish().await;
}

#[tokio::test]
async fn close_drains_a_routed_event_that_arrives_while_it_waits_for_an_inflight_append() {
    let entered = Arc::new(tokio::sync::Notify::new());
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let armed = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let (notify, wait, flag) = (entered.clone(), gate.clone(), armed);
    let hook: IoHook = Arc::new(move |r| {
        let (notify, wait, flag) = (notify.clone(), wait.clone(), flag.clone());
        Box::pin(async move {
            if r.operation == IoOperation::Write && !r.after && flag.swap(false, Ordering::SeqCst) {
                notify.notify_one();
                wait.acquire().await.unwrap().forget();
            }
            Ok(())
        })
    });
    let f = Fixture::with_hook(JsonlCompression::None, Some(hook)).await;
    let w = f.backend.create(&meta("late-close"), None).await.unwrap();
    let live =
        hoh_session_persistence_jsonl::testing::writer(&f.backend, &"late-close".into()).unwrap();
    let rows = arcs(&log());
    let append = w.append(&rows[..1], None);
    entered.notified().await;
    let closing = w.close();
    for event in &rows[1..] {
        live.enqueue_live(event, no_op_report());
    }
    gate.add_permits(1);
    append.await.unwrap();
    closing.await.unwrap();
    let r = f
        .backend
        .open(&"late-close".into(), SessionAccess::Read, None)
        .await
        .unwrap();
    assert_eq!(read(r.as_ref()).await, log());
    r.close().await.unwrap();
    f.finish().await;
}

#[tokio::test]
async fn service_flush_skips_a_write_claim_whose_handle_is_still_opening() {
    let entered = Arc::new(tokio::sync::Notify::new());
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let (notify, wait) = (entered.clone(), gate.clone());
    let hook: IoHook = Arc::new(move |r| {
        let (notify, wait) = (notify.clone(), wait.clone());
        Box::pin(async move {
            if r.operation == IoOperation::ReadFile && !r.after {
                notify.notify_one();
                wait.acquire().await.unwrap().forget();
            }
            Ok(())
        })
    });
    let f = Fixture::with_hook(JsonlCompression::None, Some(hook)).await;
    f.store(&meta("opening"), &log()).await;
    let backend = f.backend.clone();
    let open = tokio::spawn(async move {
        backend
            .open(&"opening".into(), SessionAccess::Write, None)
            .await
    });
    entered.notified().await;
    f.backend.flush().await.unwrap();
    gate.add_permits(1);
    open.await.unwrap().unwrap().close().await.unwrap();
    f.finish().await;
}

#[tokio::test(start_paused = true)]
async fn live_write_batch_uses_the_fixed_two_hundred_millisecond_window() {
    let entered = Arc::new(tokio::sync::Notify::new());
    let notify = entered.clone();
    let hook: IoHook = Arc::new(move |r| {
        let notify = notify.clone();
        Box::pin(async move {
            if r.operation == IoOperation::Write && !r.after {
                notify.notify_one();
            }
            Ok(())
        })
    });
    let f = Fixture::with_hook(JsonlCompression::None, Some(hook)).await;
    let w = f.backend.create(&meta("timer"), None).await.unwrap();
    let live = hoh_session_persistence_jsonl::testing::writer(&f.backend, &"timer".into()).unwrap();
    for e in arcs(&log()) {
        live.enqueue_live(&e, no_op_report());
    }
    tokio::task::yield_now().await;
    tokio::time::advance(std::time::Duration::from_millis(199)).await;
    assert!(!std::path::Path::new(&f.path("timer")).exists());
    tokio::time::advance(std::time::Duration::from_millis(1)).await;
    entered.notified().await;
    live.drain_live().await.unwrap();
    assert_eq!(read(w.as_ref()).await, log());
    w.close().await.unwrap();
    f.finish().await;
}

#[tokio::test]
async fn reports_the_failure_a_sequential_listing_traversal_would_reach_first() {
    let enabled = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = enabled.clone();
    let hook: IoHook = Arc::new(move |r| {
        let flag = flag.clone();
        Box::pin(async move {
            if flag.load(Ordering::SeqCst) && !r.after && r.operation == IoOperation::ReadDir {
                if r.path.ends_with("/a") {
                    tokio::task::yield_now().await;
                    return Err(Box::new(std::io::Error::other("earlier session failure"))
                        as hoh_error::BoxError);
                }
                if r.path.ends_with("--z--") {
                    return Err(Box::new(std::io::Error::other("later project failure"))
                        as hoh_error::BoxError);
                }
            }
            Ok(())
        })
    });
    let f = Fixture::with_hook(JsonlCompression::None, Some(hook)).await;
    for (id, cwd) in [("a", "/a"), ("z", "/z")] {
        let mut h = meta(id);
        h.cwd = Some(cwd.into());
        f.store(&h, &[]).await;
    }
    f.backend.list(None).await.unwrap();
    enabled.store(true, Ordering::SeqCst);
    fail(f.backend.list(None).await, "earlier session failure");
    f.finish().await;
}

#[tokio::test]
async fn rejects_a_stored_full_header_carrying_the_legacy_fallback_reason() {
    let f = Fixture::new(JsonlCompression::None).await;
    f.store(&meta("legacy-fallback"),&[event("turn/start",0,json!({"turn":1})),event("request/header",1,json!({"header":{"config":{"provider":"mock","model":"legacy"}},"reason":"fallback"}))]).await;
    fail(
        f.backend
            .open(&"legacy-fallback".into(), SessionAccess::Read, None)
            .await,
        "unsupported legacy reason \"fallback\"",
    );
    f.finish().await;
}
#[tokio::test]
async fn shares_deeply_frozen_opaque_json_from_the_current_format() {
    let f = Fixture::new(JsonlCompression::None).await;
    let data=hoh_util_values::js_json_parse("{\"nested\":[[{\"values\":[null,true,7,\"text\",[]],\"__proto__\":{\"leaf\":1},\"constructor\":{\"leaf\":2}}]]}").unwrap();
    let mut row = event("external/frozen-json", 0, data.clone());
    row["ignorable"] = true.into();
    f.store(&meta("frozen"), &[row]).await;
    let r = f
        .backend
        .open(&"frozen".into(), SessionAccess::Read, None)
        .await
        .unwrap();
    let a = r.read(None, None, None).await.unwrap();
    let b = r.read(None, None, None).await.unwrap();
    assert_eq!(a.event_state, SessionSeedEventState::SharedFrozen);
    assert_eq!(a.events[0].data(), &data);
    assert!(Arc::ptr_eq(&a.events[0], &b.events[0]));
    assert_eq!(a.events[0].data()["nested"][0][0]["__proto__"]["leaf"], 1);
    r.close().await.unwrap();
    f.finish().await;
}
#[tokio::test]
async fn resolves_absent_current_and_historical_current_generation_paths_without_publishing() {
    use hoh_session_persistence_jsonl::storage::JsonlHandleStorage;
    let f = Fixture::new(JsonlCompression::None).await;
    assert!(
        f.backend
            .resolve_current_log(&"absent".into(), None)
            .await
            .unwrap()
            .is_none()
    );
    let (current, _) = f.store(&meta("current"), &log()).await;
    assert_eq!(
        f.backend
            .resolve_current_log(&"current".into(), None)
            .await
            .unwrap(),
        Some(current)
    );
    let mut h = serde_json::to_value(meta("historical")).unwrap();
    h["version"] = 3.into();
    let (source, bytes) = f.store_raw(&h, &[]).await;
    assert!(
        f.backend
            .resolve_current_log(&"historical".into(), None)
            .await
            .unwrap()
            .is_none()
    );
    assert!(!std::path::Path::new(&f.path("historical")).exists());
    assert_eq!(tokio::fs::read(source).await.unwrap(), bytes);
    f.finish().await;
}
#[tokio::test]
async fn reports_the_selected_compressed_artifact_path_in_stat_and_list_metadata() {
    let f = Fixture::new(JsonlCompression::Zstd).await;
    let (path, bytes) = f.store(&meta("zstd-location"), &log()).await;
    let stat = f
        .backend
        .stat(&"zstd-location".into(), None)
        .await
        .unwrap()
        .unwrap();
    let listed = f.backend.list(None).await.unwrap().remove(0);
    for s in [stat, listed] {
        assert_eq!(s.location.unwrap().path, path);
        assert_eq!(s.size_bytes, Some(bytes.len() as u64));
        assert_eq!(s.format_status, Some(SessionFormatStatus::Current));
    }
    f.finish().await;
}
#[tokio::test]
async fn selects_the_highest_opposite_encoding_generation_for_its_refusal() {
    let f = Fixture::new(JsonlCompression::None).await;
    let mut highest = String::new();
    for version in [2, 4] {
        let path = generation_log_path(
            &f.root.path().to_string_lossy(),
            None,
            &"opposite".into(),
            version,
            JsonlCompression::Zstd,
        )
        .unwrap();
        tokio::fs::create_dir_all(std::path::Path::new(&path).parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(&path, b"other").await.unwrap();
        highest = path;
    }
    let e = fail(f.backend.list(None).await, "configured for compression");
    assert!(e.to_string().contains(&highest));
    f.finish().await;
}
#[tokio::test]
async fn propagates_a_non_enoent_opposite_generation_scan_failure_during_materialization() {
    let armed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = armed.clone();
    let reason =
        hoh_error::SharedError::new(std::io::Error::other("opposite-generation scan denied"));
    let failure = reason.clone();
    let hook: IoHook = Arc::new(move |r| {
        let flag = flag.clone();
        let failure = failure.clone();
        Box::pin(async move {
            if !r.after
                && r.operation == IoOperation::ReadDir
                && r.path.ends_with("/scan-failure")
                && flag.load(Ordering::SeqCst)
            {
                return Err(Box::new(failure) as hoh_error::BoxError);
            }
            Ok(())
        })
    });
    let f = Fixture::with_hook(JsonlCompression::None, Some(hook)).await;
    let w = f.backend.create(&meta("scan-failure"), None).await.unwrap();
    armed.store(true, Ordering::SeqCst);
    let e = fail(w.flush(None).await, "opposite-generation scan denied");
    assert!(
        hoh_error::SharedError::of(e.as_ref())
            .unwrap()
            .ptr_eq(&reason)
    );
    armed.store(false, Ordering::SeqCst);
    w.close().await.unwrap();
    f.finish().await;
}
#[tokio::test]
async fn surfaces_a_file_occupying_the_targeted_session_directory_path() {
    let f = Fixture::new(JsonlCompression::None).await;
    let dir = session_dir(&f.root.path().to_string_lossy(), None, &"blocked".into()).unwrap();
    tokio::fs::create_dir_all(std::path::Path::new(&dir).parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(dir, b"not a directory").await.unwrap();
    fail(
        f.backend
            .open(&"blocked".into(), SessionAccess::Read, None)
            .await,
        "ENOTDIR",
    );
    f.finish().await;
}
#[tokio::test]
async fn a_handle_read_retries_once_when_the_file_revision_changes_during_the_read() {
    let reads = Arc::new(AtomicUsize::new(0));
    let n = reads.clone();
    let hook: IoHook = Arc::new(move |r| {
        let n = n.clone();
        Box::pin(async move {
            if r.operation == IoOperation::ReadFile
                && r.after
                && n.fetch_add(1, Ordering::SeqCst) == 0
            {
                append_bytes(
                    &r.path,
                    line(&event("feedback/record", 6, json!({}))).as_bytes(),
                )
                .await;
            }
            Ok(())
        })
    });
    let f = Fixture::with_hook(JsonlCompression::None, Some(hook)).await;
    f.store(&meta("revision-race"), &log()).await;
    let r = f
        .backend
        .open(&"revision-race".into(), SessionAccess::Read, None)
        .await
        .unwrap();
    assert_eq!(read(r.as_ref()).await.len(), 7);
    assert_eq!(reads.load(Ordering::SeqCst), 2);
    r.close().await.unwrap();
    f.finish().await;
}
#[tokio::test]
async fn a_continuously_churning_revision_yields_the_committed_prefix_instead_of_looping() {
    let reads = Arc::new(AtomicUsize::new(0));
    let n = reads.clone();
    let enabled = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let active = enabled.clone();
    let hook: IoHook = Arc::new(move |r| {
        let n = n.clone();
        let active = active.clone();
        Box::pin(async move {
            if r.operation == IoOperation::ReadFile && r.after && active.load(Ordering::SeqCst) {
                n.fetch_add(1, Ordering::SeqCst);
                append_bytes(&r.path, b" ").await;
            }
            Ok(())
        })
    });
    let f = Fixture::with_hook(JsonlCompression::None, Some(hook)).await;
    f.store(&meta("revision-churn"), &log()).await;
    let r = f
        .backend
        .open(&"revision-churn".into(), SessionAccess::Read, None)
        .await
        .unwrap();
    append_bytes(&f.path("revision-churn"), b" ").await;
    enabled.store(true, Ordering::SeqCst);
    assert_eq!(read(r.as_ref()).await, log());
    r.close().await.unwrap();
    assert_eq!(reads.load(Ordering::SeqCst), 2);
    f.finish().await;
}
#[tokio::test]
async fn stat_reports_absence_for_an_artifact_vanishing_before_its_identity_stat_and_surfaces_other_faults()
 {
    use hoh_session_persistence_jsonl::testing::NodeFileError;
    let code = Arc::new(AtomicUsize::new(0));
    let fault = code.clone();
    let hook: IoHook = Arc::new(move |r| {
        let fault = fault.clone();
        Box::pin(async move {
            let errno = fault.load(Ordering::SeqCst);
            if errno != 0
                && r.operation == IoOperation::Stat
                && !r.after
                && r.path.ends_with(".jsonl")
            {
                return Err(Box::new(NodeFileError::new(
                    std::io::Error::from_raw_os_error(errno as i32),
                    "stat",
                    &r.path,
                    None,
                )) as hoh_error::BoxError);
            }
            Ok(())
        })
    });
    let f = Fixture::with_hook(JsonlCompression::None, Some(hook)).await;
    f.store(&meta("stat-fault"), &log()).await;
    code.store(2, Ordering::SeqCst);
    assert!(
        f.backend
            .stat(&"stat-fault".into(), None)
            .await
            .unwrap()
            .is_none()
    );
    code.store(13, Ordering::SeqCst);
    fail(f.backend.stat(&"stat-fault".into(), None).await, "EACCES");
    code.store(0, Ordering::SeqCst);
    f.finish().await;
}
#[tokio::test]
async fn omits_a_listed_artifact_removed_after_discovery() {
    let flag = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let armed = flag.clone();
    let hook: IoHook = Arc::new(move |r| {
        let armed = armed.clone();
        Box::pin(async move {
            if r.operation == IoOperation::Stat
                && !r.after
                && r.path.ends_with(".jsonl")
                && armed.swap(false, Ordering::SeqCst)
            {
                tokio::fs::remove_file(&r.path).await.unwrap();
            }
            Ok(())
        })
    });
    let f = Fixture::with_hook(JsonlCompression::None, Some(hook)).await;
    f.store(&meta("vanishing"), &log()).await;
    assert!(f.backend.list(None).await.unwrap().is_empty());
    f.finish().await;
}
#[tokio::test]
async fn surfaces_non_enoent_stat_failures_during_listing() {
    let hook: IoHook = Arc::new(move |r| {
        Box::pin(async move {
            if r.operation == IoOperation::Stat && !r.after && r.path.ends_with(".jsonl") {
                return Err(
                    Box::new(std::io::Error::other("snapshot stat failure")) as hoh_error::BoxError
                );
            }
            Ok(())
        })
    });
    let f = Fixture::with_hook(JsonlCompression::None, Some(hook)).await;
    f.store(&meta("stat-denied"), &log()).await;
    fail(f.backend.list(None).await, "snapshot stat failure");
    f.finish().await;
}
#[tokio::test]
async fn checks_cancellation_after_an_uncancellable_list_stat_settles() {
    let c = Arc::new(hoh_timeout::AbortController::new());
    let controller = c.clone();
    let hook: IoHook = Arc::new(move |r| {
        let controller = controller.clone();
        Box::pin(async move {
            if r.operation == IoOperation::Stat && r.after && r.path.ends_with(".jsonl") {
                controller.abort_with(std::io::Error::other("JSONL list stat cancelled"));
            }
            Ok(())
        })
    });
    let f = Fixture::with_hook(JsonlCompression::None, Some(hook)).await;
    f.store(&meta("stat-cancelled"), &log()).await;
    fail(
        f.backend
            .list(Some(&SessionPersistenceListOptions {
                signal: Some(c.signal()),
            }))
            .await,
        "JSONL list stat cancelled",
    );
    f.finish().await;
}

#[tokio::test]
async fn forwards_list_cancellation_and_awaits_inflight_discovery_cleanup() {
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let armed = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let (notice, gate, flag) = (entered.clone(), release.clone(), armed);
    let hook: IoHook = Arc::new(move |r| {
        let (notice, gate, flag) = (notice.clone(), gate.clone(), flag.clone());
        Box::pin(async move {
            if r.operation == IoOperation::Stat
                && !r.after
                && r.path.ends_with(".jsonl")
                && flag.swap(false, Ordering::SeqCst)
            {
                notice.notify_one();
                gate.acquire().await.unwrap().forget();
            }
            Ok(())
        })
    });
    let f = Fixture::with_hook(JsonlCompression::None, Some(hook)).await;
    f.store(&meta("cancel-list"), &log()).await;
    let controller = hoh_timeout::AbortController::new();
    let options = SessionPersistenceListOptions {
        signal: Some(controller.signal()),
    };
    let backend = f.backend.clone();
    let pending = tokio::spawn(async move { backend.list(Some(&options)).await });
    entered.notified().await;
    controller.abort_with(std::io::Error::other("JSONL list discovery cancelled"));
    tokio::task::yield_now().await;
    assert!(!pending.is_finished());
    release.add_permits(1);
    fail(pending.await.unwrap(), "JSONL list discovery cancelled");
    f.finish().await;
}
#[tokio::test]
async fn backend_dispose_aggregates_open_handle_close_failures() {
    let armed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = armed.clone();
    let hook: IoHook = Arc::new(move |r| {
        let flag = flag.clone();
        Box::pin(async move {
            if r.operation == IoOperation::Close
                && r.after
                && r.path.ends_with("session.lock")
                && flag.swap(false, Ordering::SeqCst)
            {
                return Err(
                    Box::new(std::io::Error::other("close exploded")) as hoh_error::BoxError
                );
            }
            Ok(())
        })
    });
    let f = Fixture::with_hook(JsonlCompression::None, Some(hook)).await;
    let w = f.backend.create(&meta("dispose-fail"), None).await.unwrap();
    w.append(&arcs(&log()), None).await.unwrap();
    armed.store(true, Ordering::SeqCst);
    let e = fail(
        f.backend.close().await,
        "session-persistence-jsonl dispose failed",
    );
    let hoh_session_persistence_jsonl::JsonlError::Aggregate { errors, .. } = e
        .downcast_ref::<hoh_session_persistence_jsonl::JsonlError>()
        .unwrap()
    else {
        panic!("aggregate")
    };
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].to_string(), "close exploded");
    f.finish().await;
}
#[tokio::test]
async fn create_snapshots_its_metadata_before_materialization() {
    let f = Fixture::new(JsonlCompression::None).await;
    let mut m = meta("create-snap");
    m.cwd = Some("/orig".into());
    let w = f.backend.create(&m, None).await.unwrap();
    m.cwd = Some("/mutated".into());
    w.append(&arcs(&log()), None).await.unwrap();
    w.close().await.unwrap();
    let original = log_path(
        &f.root.path().to_string_lossy(),
        Some("/orig"),
        &m.id,
        JsonlCompression::None,
    )
    .unwrap();
    let modified = log_path(
        &f.root.path().to_string_lossy(),
        m.cwd.as_deref(),
        &m.id,
        JsonlCompression::None,
    )
    .unwrap();
    assert!(std::path::Path::new(&original).is_file());
    assert!(!std::path::Path::new(&modified).exists());
    f.finish().await;
}

#[tokio::test]
async fn tracks_a_disappearing_corpus_member_and_propagates_storage_faults_in_historical_revisions()
{
    let mode = Arc::new(AtomicUsize::new(0));
    let current = mode.clone();
    let reason = hoh_error::SharedError::new(std::io::Error::other("member denied"));
    let denied = reason.clone();
    let hook: IoHook = Arc::new(move |r| {
        let current = current.clone();
        let denied = denied.clone();
        Box::pin(async move {
            if r.operation == IoOperation::Stat
                && !r.after
                && r.path.ends_with("/child/session.v3.jsonl")
            {
                match current.load(Ordering::SeqCst) {
                    2 => {
                        return Err(Box::new(
                            hoh_session_persistence_jsonl::testing::NodeFileError::new(
                                std::io::Error::from_raw_os_error(2),
                                "stat",
                                &r.path,
                                None,
                            ),
                        ) as hoh_error::BoxError);
                    }
                    13 => return Err(Box::new(denied) as hoh_error::BoxError),
                    _ => {}
                }
            }
            Ok(())
        })
    });
    let f = Fixture::with_hook(JsonlCompression::None, Some(hook)).await;
    for id in ["parent", "child"] {
        let mut h = serde_json::to_value(meta(id)).unwrap();
        h["version"] = 3.into();
        f.store_raw(&h, &[]).await;
    }
    let present = f
        .backend
        .stat(&"parent".into(), None)
        .await
        .unwrap()
        .unwrap()
        .revision;
    mode.store(2, Ordering::SeqCst);
    let missing = f
        .backend
        .stat(&"parent".into(), None)
        .await
        .unwrap()
        .unwrap()
        .revision;
    assert_ne!(present, missing);
    assert_eq!(
        missing,
        f.backend
            .stat(&"parent".into(), None)
            .await
            .unwrap()
            .unwrap()
            .revision
    );
    mode.store(13, Ordering::SeqCst);
    let error = fail(
        f.backend.stat(&"parent".into(), None).await,
        "member denied",
    );
    assert!(
        hoh_error::SharedError::of(error.as_ref())
            .unwrap()
            .ptr_eq(&reason)
    );
    mode.store(0, Ordering::SeqCst);
    f.finish().await;
}

#[tokio::test]
async fn trash_moves_complete_directory_and_excludes_it_from_list() {
    let f = Fixture::new(JsonlCompression::None).await;
    let id = SessionId::from("delete-me");
    let (path, bytes) = f.store(&meta("delete-me"), &close_turn()).await;
    let dir = std::path::Path::new(&path).parent().unwrap();
    tokio::fs::write(dir.join("attachment.dat"), b"keep me")
        .await
        .unwrap();
    assert_eq!(f.backend.list(None).await.unwrap().len(), 1);
    f.backend.trash(&id).await.unwrap();
    assert!(f.backend.list(None).await.unwrap().is_empty());
    assert!(f.backend.stat(&id, None).await.unwrap().is_none());
    let mut entries = tokio::fs::read_dir(f.root.path().join("trash"))
        .await
        .unwrap();
    let trashed = entries.next_entry().await.unwrap().unwrap().path();
    assert_eq!(
        tokio::fs::read(trashed.join("attachment.dat"))
            .await
            .unwrap(),
        b"keep me"
    );
    assert_eq!(
        tokio::fs::read(trashed.join(std::path::Path::new(&path).file_name().unwrap()))
            .await
            .unwrap(),
        bytes
    );
    assert!(entries.next_entry().await.unwrap().is_none());
    f.backend.trash(&id).await.unwrap();
    f.finish().await;
}

#[tokio::test]
async fn trash_refuses_a_live_writer() {
    let f = Fixture::new(JsonlCompression::None).await;
    f.store(&meta("busy-delete"), &close_turn()).await;
    let id = SessionId::from("busy-delete");
    let writer = f
        .backend
        .open(&id, SessionAccess::Write, None)
        .await
        .unwrap();
    assert!(f.backend.trash(&id).await.is_err());
    assert!(f.backend.stat(&id, None).await.unwrap().is_some());
    writer.close().await.unwrap();
    f.backend.trash(&id).await.unwrap();
    f.finish().await;
}

#[cfg(unix)]
#[tokio::test]
async fn del2_lock_symlink_never_changes_the_referenced_file() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new(JsonlCompression::None).await;
    let (path, _) = f.store(&meta("lock-link"), &close_turn()).await;
    let victim = f.root.path().join("victim");
    tokio::fs::write(&victim, b"untouched").await.unwrap();
    symlink(
        &victim,
        std::path::Path::new(&path)
            .parent()
            .unwrap()
            .join("session.lock"),
    )
    .unwrap();
    assert!(f.backend.trash(&"lock-link".into()).await.is_err());
    assert_eq!(tokio::fs::read(victim).await.unwrap(), b"untouched");
    f.finish().await;
}

#[cfg(unix)]
#[tokio::test]
async fn del2_trash_symlink_is_refused_without_moving_data() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new(JsonlCompression::None).await;
    let external = tempfile::tempdir().unwrap();
    let (path, bytes) = f.store(&meta("trash-link"), &close_turn()).await;
    symlink(external.path(), f.root.path().join("trash")).unwrap();
    assert!(f.backend.trash(&"trash-link".into()).await.is_err());
    assert_eq!(tokio::fs::read(path).await.unwrap(), bytes);
    assert!(std::fs::read_dir(external.path()).unwrap().next().is_none());
    f.finish().await;
}

#[tokio::test]
async fn del2_trash_rejects_a_log_with_a_different_header_id() {
    let f = Fixture::new(JsonlCompression::None).await;
    let (path, bytes) = f.store(&meta("actual"), &close_turn()).await;
    let source = std::path::Path::new(&path).parent().unwrap();
    let wrong = source.parent().unwrap().join("requested");
    tokio::fs::rename(source, &wrong).await.unwrap();
    assert!(f.backend.trash(&"requested".into()).await.is_err());
    assert_eq!(
        tokio::fs::read(wrong.join(std::path::Path::new(&path).file_name().unwrap()))
            .await
            .unwrap(),
        bytes
    );
    f.finish().await;
}

#[tokio::test]
async fn del2_inventory_ignores_a_nested_foreign_trash_but_keeps_a_chat_named_trash() {
    let f = Fixture::new(JsonlCompression::None).await;
    let (foreign_path, _) = f.store(&meta("foreign"), &close_turn()).await;
    let project = std::path::Path::new(&foreign_path)
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let nested = project.join("trash");
    tokio::fs::rename(
        std::path::Path::new(&foreign_path).parent().unwrap(),
        &nested,
    )
    .await
    .unwrap();
    assert!(f.backend.list(None).await.unwrap().is_empty());
    tokio::fs::remove_dir_all(&nested).await.unwrap();
    f.store(&meta("trash"), &close_turn()).await;
    assert_eq!(
        f.backend.list(None).await.unwrap()[0].header.id.as_str(),
        "trash"
    );
    f.finish().await;
}
