mod common;
use common::*;
use hoh_session_persistence::{SessionAccess, SessionPersistence};
use hoh_session_persistence_jsonl::zstd::*;
use hoh_session_persistence_jsonl::{JsonlCompression, format::to_header_line};
use serde_json::json;
const MAGIC: [u8; 4] = [0x28, 0xb5, 0x2f, 0xfd];
#[tokio::test]
async fn scans_concatenated_checksummed_frames_and_honors_a_frame_limit() {
    let first = compress_zstd_frame("header\n").await.unwrap();
    let second = compress_zstd_frame("event\n").await.unwrap();
    let stream = [first.clone(), second.clone()].concat();
    assert_eq!(
        scan_zstd_frames(&[], None).unwrap(),
        ZstdFrameScan::default()
    );
    let scan = scan_zstd_frames(&stream, None).unwrap();
    assert_eq!(
        scan.frames,
        vec![
            ZstdFrameRange {
                start: 0,
                end: first.len()
            },
            ZstdFrameRange {
                start: first.len(),
                end: stream.len()
            }
        ]
    );
    assert_eq!(
        scan_zstd_frames(&stream, Some(1)).unwrap().frames,
        scan.frames[..1]
    );
    assert_eq!(first[4] & 4, 4);
    assert_eq!(second[4] & 4, 4);
    assert_eq!(decompress_zstd_frame(&first).await.unwrap(), b"header\n");
    let mut decoder = create_zstd_frame_decoder();
    assert_eq!(
        decoder
            .decode(&stream, &scan.frames)
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
            .concat(),
        b"header\nevent\n"
    );
}
#[tokio::test]
async fn keeps_the_public_and_native_synchronous_decoders_interchangeable() {
    let stream = [
        compress_zstd_frame("first\n").await.unwrap(),
        compress_zstd_frame("second\n").await.unwrap(),
    ]
    .concat();
    let ranges = scan_zstd_frames(&stream, None).unwrap().frames;
    let decoders = [
        hoh_session_persistence_jsonl::zstd_public_decoder::PublicZstdFrameDecoder::new(),
        hoh_session_persistence_jsonl::zstd_private_decoder::NodePrivateZstdFrameDecoder::create()
            .unwrap(),
    ];
    for mut decoder in decoders {
        let plaintext = decoder
            .decode(&stream, &ranges)
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(plaintext.len(), 2);
        assert_eq!(plaintext.concat(), b"first\nsecond\n");
        decoder.close();
    }
}
#[tokio::test]
async fn enforces_decoder_lifecycle_and_checksum_errors_through_both_implementations() {
    let frame = compress_zstd_frame("frame\n").await.unwrap();
    let ranges = [ZstdFrameRange {
        start: 0,
        end: frame.len(),
    }];
    let mut corrupt = frame.clone();
    *corrupt.last_mut().unwrap() ^= 255;
    for _ in 0..2 {
        let mut interrupted = create_zstd_frame_decoder();
        {
            let mut iterator = interrupted.decode(&frame, &ranges).unwrap();
            assert_eq!(iterator.next().unwrap().unwrap(), b"frame\n");
        }
        assert!(
            interrupted
                .decode(&frame, &ranges)
                .err()
                .unwrap()
                .to_string()
                .contains("already started")
        );
        interrupted.close();
        let mut closed = create_zstd_frame_decoder();
        closed.close();
        closed.close();
        assert!(
            closed
                .decode(&frame, &ranges)
                .err()
                .unwrap()
                .to_string()
                .contains("closed")
        );
        let mut invalid = create_zstd_frame_decoder();
        fail(
            invalid
                .decode(&corrupt, &ranges)
                .unwrap()
                .collect::<Result<Vec<_>, _>>(),
            "frame at byte 0 failed validation",
        );
    }
}
#[tokio::test]
async fn assembles_native_decoder_output_at_and_beyond_its_reusable_chunk_boundary() {
    for length in [8, 9] {
        let plaintext = vec![b'a'; length];
        let frame = compress_zstd_frame(&plaintext).await.unwrap();
        let range = [ZstdFrameRange {
            start: 0,
            end: frame.len(),
        }];
        let mut decoder = ZstdFrameDecoder::with_chunk_size(8);
        assert_eq!(
            decoder
                .decode(&frame, &range)
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap(),
            vec![plaintext]
        );
    }
}
#[test]
fn distinguishes_incomplete_frame_regions_from_invalid_complete_structure() {
    let torn = |bytes: &[u8]| {
        assert_eq!(
            scan_zstd_frames(bytes, None).unwrap(),
            ZstdFrameScan {
                frames: vec![],
                torn_start: Some(0)
            }
        )
    };
    torn(&MAGIC[..2]);
    torn(&MAGIC);
    fail(scan_zstd_frames(&[0; 4], None), "invalid frame magic");
    fail(
        scan_zstd_frames(&[&MAGIC[..], &[8]].concat(), None),
        "reserved frame-header bit",
    );
    torn(&[&MAGIC[..], &[0]].concat());
    torn(&[&MAGIC[..], &[0x20, 0, 1, 0]].concat());
    torn(&[&MAGIC[..], &[0x20, 0, 41, 0, 0, 1, 2]].concat());
    fail(
        scan_zstd_frames(&[&MAGIC[..], &[0x20, 0, 7, 0, 0]].concat(), None),
        "reserved block type",
    );
}
fn structural_frame(descriptor: u8) -> Vec<u8> {
    let mut bytes = MAGIC.to_vec();
    bytes.push(descriptor);
    let single = descriptor & 0x20 != 0;
    let dict = descriptor & 3;
    let dictionary = if dict == 3 { 4 } else { dict as usize };
    let flag = descriptor >> 6;
    let size = if flag == 0 {
        usize::from(single)
    } else {
        1 << flag
    };
    bytes.resize(bytes.len() + usize::from(!single) + dictionary + size, 0);
    bytes.extend([1, 0, 0]);
    if descriptor & 4 != 0 {
        bytes.extend([0; 4]);
    }
    bytes
}
#[test]
fn covers_standard_header_variants_rle_blocks_multiple_blocks_and_checksums() {
    for descriptor in [0, 0x21, 0x42, 0x83, 0xe3] {
        let frame = structural_frame(descriptor);
        assert_eq!(
            scan_zstd_frames(&frame, None).unwrap().frames,
            vec![ZstdFrameRange {
                start: 0,
                end: frame.len()
            }]
        );
    }
    for tail in [
        &[0x20, 1, 11, 0, 0, 65][..],
        &[0x20, 0, 0, 0, 0, 1, 0, 0][..],
    ] {
        let frame = [&MAGIC[..], tail].concat();
        assert_eq!(
            scan_zstd_frames(&frame, None).unwrap().frames[0].end,
            frame.len()
        );
    }
    let checksummed = structural_frame(0x24);
    assert_eq!(
        scan_zstd_frames(&checksummed[..checksummed.len() - 1], None)
            .unwrap()
            .torn_start,
        Some(0)
    );
    assert_eq!(
        scan_zstd_frames(&checksummed, None).unwrap().frames[0].end,
        checksummed.len()
    );
}
#[tokio::test]
async fn materializes_an_explicitly_durable_empty_session_as_one_header_frame() {
    let f = Fixture::new(JsonlCompression::Zstd).await;
    let h = f.backend.create(&meta("empty-zstd"), None).await.unwrap();
    h.flush(None).await.unwrap();
    h.close().await.unwrap();
    let bytes = tokio::fs::read(f.path("empty-zstd")).await.unwrap();
    assert_eq!(scan_zstd_frames(&bytes, None).unwrap().frames.len(), 1);
    assert_eq!(
        decompress_zstd_frame(&bytes).await.unwrap(),
        line(&to_header_line(&meta("empty-zstd"), None).unwrap()).as_bytes()
    );
    f.finish().await;
}
#[tokio::test]
async fn writes_zstd_by_default_with_one_header_frame_and_one_first_batch_frame() {
    assert_eq!(
        hoh_session_persistence_jsonl::Config::new("root").compression,
        JsonlCompression::Zstd
    );
    let f = Fixture::new(JsonlCompression::Zstd).await;
    let h = f.backend.create(&meta("default-zstd"), None).await.unwrap();
    h.append(&arcs(&log()), None).await.unwrap();
    h.close().await.unwrap();
    let bytes = tokio::fs::read(f.path("default-zstd")).await.unwrap();
    let frames = scan_zstd_frames(&bytes, None).unwrap();
    assert_eq!(&bytes[..4], MAGIC);
    assert_eq!(frames.frames.len(), 2);
    let mut decoded = vec![];
    for frame in frames.frames {
        decoded.extend(
            decompress_zstd_frame(&bytes[frame.start..frame.end])
                .await
                .unwrap(),
        );
    }
    let mut expected = line(&to_header_line(&meta("default-zstd"), None).unwrap());
    for row in log() {
        expected.push_str(&line(&row));
    }
    assert_eq!(decoded, expected.as_bytes());
    f.finish().await;
}
#[tokio::test]
async fn appends_one_frame_per_durable_batch_without_rewriting_prior_bytes() {
    let f = Fixture::new(JsonlCompression::Zstd).await;
    let h = f.backend.create(&meta("append-frame"), None).await.unwrap();
    h.append(&arcs(&log()), None).await.unwrap();
    let before = tokio::fs::read(f.path("append-frame")).await.unwrap();
    h.append(
        &arcs(&[
            event("turn/start", 6, json!({"turn":2})),
            event(
                "turn/end",
                7,
                json!({"turn":2,"reason":{"kind":"completed"}}),
            ),
        ]),
        None,
    )
    .await
    .unwrap();
    let bytes = tokio::fs::read(f.path("append-frame")).await.unwrap();
    assert!(bytes.starts_with(&before));
    assert_eq!(scan_zstd_frames(&bytes, None).unwrap().frames.len(), 3);
    h.close().await.unwrap();
    f.finish().await;
}
#[tokio::test]
async fn lists_a_multi_chunk_header_frame_without_decoding_a_corrupt_event_frame() {
    let f = Fixture::new(JsonlCompression::Zstd).await;
    let mut m = meta("large-header");
    m.cwd = Some(format!("/work/{}", "x".repeat(24000)));
    let (path, mut bytes) = f.store(&m, &log()).await;
    let frame = scan_zstd_frames(&bytes, None).unwrap().frames[1];
    bytes[frame.end - 1] ^= 255;
    tokio::fs::write(&path, &bytes).await.unwrap();
    assert_eq!(f.backend.list(None).await.unwrap().len(), 1);
    assert!(f.backend.stat(&m.id, None).await.unwrap().is_some());
    fail_match(
        f.backend.open(&m.id, SessionAccess::Read, None).await,
        "frame at byte [0-9]+ failed validation",
    );
    f.finish().await;
}
#[tokio::test]
async fn drops_a_frame_torn_in_its_header_before_it_has_produced_plaintext() {
    let f = Fixture::new(JsonlCompression::Zstd).await;
    let (path, bytes) = f.store(&meta("partial-magic"), &log()).await;
    append_bytes(&path, &MAGIC[..2]).await;
    let r = f
        .backend
        .open(&"partial-magic".into(), SessionAccess::Read, None)
        .await
        .unwrap();
    assert_eq!(read(r.as_ref()).await, log());
    assert_eq!(
        tokio::fs::read(path).await.unwrap(),
        [bytes, MAGIC[..2].to_vec()].concat()
    );
    r.close().await.unwrap();
    f.finish().await;
}
#[tokio::test]
async fn recovers_a_final_frame_torn_at_its_checksum_byte_in_full() {
    let f = Fixture::new(JsonlCompression::Zstd).await;
    let (path, committed) = f.store(&meta("partial-checksum"), &log()).await;
    let second = vec![
        event("turn/start", 6, json!({"turn":2})),
        event(
            "turn/end",
            7,
            json!({"turn":2,"reason":{"kind":"completed"}}),
        ),
    ];
    let frame = compress_zstd_frame(second.iter().map(line).collect::<String>())
        .await
        .unwrap();
    append_bytes(&path, &frame[..frame.len() - 1]).await;
    let r = f
        .backend
        .open(&"partial-checksum".into(), SessionAccess::Read, None)
        .await
        .unwrap();
    let mut expected = log();
    expected.extend(second);
    assert_eq!(read(r.as_ref()).await, expected);
    r.close().await.unwrap();
    let w = f
        .backend
        .open(&"partial-checksum".into(), SessionAccess::Write, None)
        .await
        .unwrap();
    w.append(
        &arcs(&[
            event("turn/start", 8, json!({"turn":3})),
            event(
                "turn/end",
                9,
                json!({"turn":3,"reason":{"kind":"completed"}}),
            ),
        ]),
        None,
    )
    .await
    .unwrap();
    w.close().await.unwrap();
    let bytes = tokio::fs::read(&path).await.unwrap();
    assert!(bytes.starts_with(&committed));
    assert!(scan_zstd_frames(&bytes, None).unwrap().torn_start.is_none());
    let r = f
        .backend
        .open(&"partial-checksum".into(), SessionAccess::Read, None)
        .await
        .unwrap();
    assert_eq!(read(r.as_ref()).await.len(), 10);
    r.close().await.unwrap();
    f.finish().await;
}
#[tokio::test]
async fn rejects_a_complete_frame_containing_a_torn_jsonl_record() {
    let f = Fixture::new(JsonlCompression::Zstd).await;
    let (path, _) = f.store(&meta("complete-bad"), &log()).await;
    append_bytes(
        &path,
        &compress_zstd_frame("{\"type\":\"turn/start\"")
            .await
            .unwrap(),
    )
    .await;
    fail(
        f.backend
            .open(&"complete-bad".into(), SessionAccess::Read, None)
            .await,
        "complete frame contains a torn JSONL record",
    );
    f.finish().await;
}
#[tokio::test]
async fn skips_unreadable_compressed_headers_during_discovery_while_rejecting_targeted_reads() {
    let f = Fixture::new(JsonlCompression::Zstd).await;
    for (id, bytes) in [
        ("empty", vec![]),
        ("partial", MAGIC.to_vec()),
        (
            "not-header",
            compress_zstd_frame("{\"type\":\"turn/start\"}\n")
                .await
                .unwrap(),
        ),
        ("two-lines", compress_zstd_frame("{}\n{}\n").await.unwrap()),
    ] {
        let (path, _) = f.store(&meta(id), &[]).await;
        tokio::fs::write(path, bytes).await.unwrap();
    }
    assert!(f.backend.list(None).await.unwrap().is_empty());
    fail(
        f.backend
            .open(&"two-lines".into(), SessionAccess::Read, None)
            .await,
        "first frame is not exactly one header line",
    );
    f.finish().await;
}
#[tokio::test]
async fn rejects_missing_empty_and_checksum_corrupt_header_frames_on_targeted_reads() {
    let f = Fixture::new(JsonlCompression::Zstd).await;
    let mut corrupt =
        compress_zstd_frame(line(&to_header_line(&meta("bad-checksum"), None).unwrap()))
            .await
            .unwrap();
    *corrupt.last_mut().unwrap() ^= 255;
    for (id, bytes, reason) in [
        (
            "partial-only",
            MAGIC.to_vec(),
            "empty or header-less Zstandard session log",
        ),
        (
            "empty-header",
            compress_zstd_frame("").await.unwrap(),
            "first frame is not exactly one header line",
        ),
        ("bad-checksum", corrupt, "frame at byte 0 failed validation"),
    ] {
        let (path, _) = f.store(&meta(id), &[]).await;
        tokio::fs::write(path, bytes).await.unwrap();
        fail(
            f.backend.open(&id.into(), SessionAccess::Read, None).await,
            reason,
        );
    }
    fail(
        f.backend.stat(&"bad-checksum".into(), None).await,
        "header frame failed validation",
    );
    assert!(f.backend.list(None).await.unwrap().is_empty());
    f.finish().await;
}
#[tokio::test]
async fn rejects_roots_owned_by_the_opposite_encoding_in_both_directions() {
    for compression in [JsonlCompression::None, JsonlCompression::Zstd] {
        let f = Fixture::new(compression).await;
        f.store(&meta("encoding"), &log()).await;
        let mut config =
            hoh_session_persistence_jsonl::Config::new(f.root.path().to_string_lossy());
        config.compression = if compression == JsonlCompression::None {
            JsonlCompression::Zstd
        } else {
            JsonlCompression::None
        };
        let other =
            hoh_session_persistence_jsonl::JsonlSessionPersistence::new(&f.kernel.root(), config)
                .await
                .unwrap();
        fail(other.list(None).await, "configured for compression");
        other.close().await.unwrap();
        f.finish().await;
    }
}
#[tokio::test]
async fn rechecks_targeted_artifacts_and_listing_after_an_initially_empty_root() {
    let f = Fixture::new(JsonlCompression::Zstd).await;
    assert!(f.backend.list(None).await.unwrap().is_empty());
    let path = hoh_session_persistence_jsonl::format::log_path(
        &f.root.path().to_string_lossy(),
        None,
        &"late-raw".into(),
        JsonlCompression::None,
    )
    .unwrap();
    tokio::fs::create_dir_all(std::path::Path::new(&path).parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(
        path,
        line(&to_header_line(&meta("late-raw"), None).unwrap()),
    )
    .await
    .unwrap();
    for access in [SessionAccess::Read, SessionAccess::Write] {
        fail(
            f.backend.open(&"late-raw".into(), access, None).await,
            "uses .jsonl",
        );
    }
    fail(f.backend.list(None).await, "uses .jsonl");
    f.finish().await;
}
#[tokio::test]
async fn refuses_materialization_when_an_opposite_artifact_appears_after_create() {
    let f = Fixture::new(JsonlCompression::Zstd).await;
    let h = f.backend.create(&meta("late"), None).await.unwrap();
    let path = hoh_session_persistence_jsonl::format::log_path(
        &f.root.path().to_string_lossy(),
        None,
        &"late".into(),
        JsonlCompression::None,
    )
    .unwrap();
    tokio::fs::create_dir_all(std::path::Path::new(&path).parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(path, line(&to_header_line(&meta("late"), None).unwrap()))
        .await
        .unwrap();
    fail(h.append(&arcs(&log()), None).await, "uses .jsonl");
    assert!(!std::path::Path::new(&f.path("late")).exists());
    h.close().await.unwrap();
    f.finish().await;
}

#[tokio::test(start_paused = true)]
async fn stops_multi_frame_inspection_when_cancellation_arrives_at_a_slice_deadline() {
    let f = Fixture::new(JsonlCompression::Zstd).await;
    let (_, bytes) = f.store(&meta("cancel-frames"), &log()).await;
    let c = hoh_timeout::AbortController::new();
    let signal = c.signal();
    let backend = f.backend.clone();
    let pending =
        hoh_session_persistence_jsonl::testing::read_zstd_prefix(&backend, &bytes, Some(&signal));
    tokio::pin!(pending);
    assert!(
        std::future::poll_fn(|cx| std::task::Poll::Ready(std::future::Future::poll(
            pending.as_mut(),
            cx
        )))
        .await
        .is_pending()
    );
    tokio::time::advance(std::time::Duration::from_millis(501)).await;
    c.abort_with(std::io::Error::other(
        "cancel after Zstandard decode starts",
    ));
    fail(pending.await, "cancel after Zstandard decode starts");
    f.finish().await;
}
#[tokio::test(start_paused = true)]
async fn continues_decoding_every_frame_after_a_slice_deadline_yields() {
    let f = Fixture::new(JsonlCompression::Zstd).await;
    let (_, bytes) = f.store(&meta("yield-frames"), &log()).await;
    let backend = f.backend.clone();
    let pending = hoh_session_persistence_jsonl::testing::read_zstd_prefix(&backend, &bytes, None);
    tokio::pin!(pending);
    assert!(
        std::future::poll_fn(|cx| std::task::Poll::Ready(std::future::Future::poll(
            pending.as_mut(),
            cx
        )))
        .await
        .is_pending()
    );
    tokio::time::advance(std::time::Duration::from_millis(501)).await;
    assert_eq!(
        pending
            .await
            .unwrap()
            .events
            .iter()
            .map(|e| e.as_value().clone())
            .collect::<Vec<_>>(),
        log()
    );
    f.finish().await;
}
#[tokio::test]
async fn observes_cancellation_after_each_async_raw_and_zstd_header_read_during_listing() {
    for compression in [JsonlCompression::None, JsonlCompression::Zstd] {
        let c = std::sync::Arc::new(hoh_timeout::AbortController::new());
        let controller = c.clone();
        let hook: hoh_session_persistence_jsonl::testing::IoHook = std::sync::Arc::new(move |r| {
            let controller = controller.clone();
            Box::pin(async move {
                if r.operation == hoh_session_persistence_jsonl::testing::IoOperation::Read
                    && r.after
                {
                    controller.abort_with(std::io::Error::other("cancel header read"));
                }
                Ok(())
            })
        });
        let f = Fixture::with_hook(compression, Some(hook)).await;
        f.store(&meta("cancel-header"), &log()).await;
        fail(
            f.backend
                .list(Some(
                    &hoh_session_persistence::SessionPersistenceListOptions {
                        signal: Some(c.signal()),
                    },
                ))
                .await,
            "cancel header read",
        );
        f.finish().await;
    }
}
fn noise(n: usize) -> String {
    let mut state = 0x12345678u32;
    (0..n)
        .map(|_| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            char::from(33 + (state % 90) as u8)
        })
        .collect()
}
async fn torn_with_partial_record() -> Vec<u8> {
    let rows = [
        event("turn/start", 6, json!({"turn":2})),
        event("step/start", 7, json!({"turn":2,"step":1})),
        event(
            "assistant/attempt",
            8,
            json!({"turn":2,"step":1,"stream":[{"type":"text-chunks","time0":9,"index":0,"dt":[],"texts":[noise(300000)]}]}),
        ),
    ];
    let frame = compress_zstd_frame(rows.iter().map(line).collect::<String>())
        .await
        .unwrap();
    let ends = [
        frame.len() - 1,
        frame.len() - 4,
        frame.len() * 9 / 10,
        frame.len() * 3 / 4,
        frame.len() * 6 / 10,
        frame.len() / 2,
        frame.len() * 4 / 10,
        frame.len() / 4,
    ];
    for end in ends {
        let bytes = &frame[..end];
        if scan_zstd_frames(bytes, None).unwrap().torn_start != Some(0) {
            continue;
        }
        if let Ok(decoded) = decompress_zstd_prefix(bytes).await
            && decoded.iter().filter(|b| **b == b'\n').count() >= 2
            && decoded.last() != Some(&b'\n')
        {
            return bytes.to_vec();
        }
    }
    panic!("could not produce a torn partial-record frame");
}
async fn repair_partial_frame(retry: bool) {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    let armed = Arc::new(AtomicBool::new(false));
    let flag = armed.clone();
    let hook: hoh_session_persistence_jsonl::testing::IoHook = Arc::new(move |r| {
        let flag = flag.clone();
        Box::pin(async move {
            if r.operation == hoh_session_persistence_jsonl::testing::IoOperation::Write
                && !r.after
                && r.path.ends_with(".zstd")
                && flag.swap(false, Ordering::SeqCst)
            {
                return Err(
                    Box::new(std::io::Error::other("rewrite refused")) as hoh_error::BoxError
                );
            }
            Ok(())
        })
    });
    let f = Fixture::with_hook(JsonlCompression::Zstd, Some(hook)).await;
    let (path, committed) = f.store(&meta("recover-torn"), &log()).await;
    append_bytes(&path, &torn_with_partial_record().await).await;
    let w = f
        .backend
        .open(&"recover-torn".into(), SessionAccess::Write, None)
        .await
        .unwrap();
    assert_eq!(read(w.as_ref()).await.len(), 8);
    let rows = arcs(&[
        event("step/end", 8, json!({"turn":2,"step":1})),
        event(
            "turn/end",
            9,
            json!({"turn":2,"reason":{"kind":"interrupted"}}),
        ),
    ]);
    if retry {
        armed.store(true, Ordering::SeqCst);
        fail(w.append(&rows, None).await, "rewrite refused");
    }
    w.append(&rows, None).await.unwrap();
    w.close().await.unwrap();
    let bytes = tokio::fs::read(&path).await.unwrap();
    assert!(bytes.starts_with(&committed));
    assert!(scan_zstd_frames(&bytes, None).unwrap().torn_start.is_none());
    let r = f
        .backend
        .open(&"recover-torn".into(), SessionAccess::Read, None)
        .await
        .unwrap();
    assert_eq!(read(r.as_ref()).await.len(), 10);
    r.close().await.unwrap();
    f.finish().await;
}
#[tokio::test]
async fn recovers_complete_records_from_a_torn_final_frame_and_rewrites_them_on_the_next_append() {
    repair_partial_frame(false).await;
}
#[tokio::test]
async fn retries_the_torn_tail_rewrite_when_its_first_durable_write_fails() {
    repair_partial_frame(true).await;
}
#[tokio::test]
async fn rolls_back_a_checksummed_append_frame_when_fsync_fails() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    let armed = Arc::new(AtomicBool::new(false));
    let flag = armed.clone();
    let hook: hoh_session_persistence_jsonl::testing::IoHook = Arc::new(move |r| {
        let flag = flag.clone();
        Box::pin(async move {
            if r.operation == hoh_session_persistence_jsonl::testing::IoOperation::Sync
                && !r.after
                && r.path.ends_with(".zstd")
                && flag.swap(false, Ordering::SeqCst)
            {
                return Err(
                    Box::new(std::io::Error::other("simulated Zstandard fsync failure"))
                        as hoh_error::BoxError,
                );
            }
            Ok(())
        })
    });
    let f = Fixture::with_hook(JsonlCompression::Zstd, Some(hook)).await;
    let (path, bytes) = f.store(&meta("zstd-rollback"), &log()).await;
    let w = f
        .backend
        .open(&"zstd-rollback".into(), SessionAccess::Write, None)
        .await
        .unwrap();
    let rows = arcs(&[
        event("turn/start", 6, json!({"turn":2})),
        event(
            "turn/end",
            7,
            json!({"turn":2,"reason":{"kind":"completed"}}),
        ),
    ]);
    armed.store(true, Ordering::SeqCst);
    fail(
        w.append(&rows, None).await,
        "simulated Zstandard fsync failure",
    );
    assert_eq!(tokio::fs::read(&path).await.unwrap(), bytes);
    w.append(&rows, None).await.unwrap();
    assert_eq!(read(w.as_ref()).await.len(), 8);
    w.close().await.unwrap();
    f.finish().await;
}
