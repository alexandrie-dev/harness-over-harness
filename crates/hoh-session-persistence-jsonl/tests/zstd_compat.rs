use hoh_session_persistence_jsonl::zstd::*;
#[tokio::test]
async fn round_trips_concatenated_checksummed_frames_through_the_native_api() {
    let mut encoded = compress_zstd_frame(
        "{\"type\":\"session\",\"version\":0,\"id\":\"compat\",\"createdAt\":1}\n",
    )
    .await
    .unwrap();
    encoded.extend(
        compress_zstd_frame("{\"type\":\"turn/start\",\"seq\":0,\"turn\":1}\n")
            .await
            .unwrap(),
    );
    let scan = scan_zstd_frames(&encoded, None).unwrap();
    assert_eq!(scan.torn_start, None);
    assert_eq!(scan.frames.len(), 2);
    for frame in &scan.frames {
        assert_eq!(
            &encoded[frame.start..frame.start + 4],
            [0x28, 0xb5, 0x2f, 0xfd]
        );
    }
    let mut decoder = create_zstd_frame_decoder();
    let decoded = decoder
        .decode(&encoded, &scan.frames)
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
        .concat();
    assert!(
        String::from_utf8(decoded)
            .unwrap()
            .contains("\"type\":\"turn/start\"")
    );
    let frame = scan.frames[1];
    assert_eq!(
        scan_zstd_frames(&encoded[frame.start..frame.end - 1], None).unwrap(),
        ZstdFrameScan {
            frames: vec![],
            torn_start: Some(0)
        }
    );
}
