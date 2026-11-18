use crate::{
    file_store::*,
    store::{permissions, publish_immutable_alias},
    test_support::*,
};
use futures_util::{StreamExt, stream};
use hoh_attachment::{FileAttachmentRef, SaveFileAttachment, SaveFileStreamAttachment};
use hoh_error::{BoxError, SharedError};
use hoh_timeout::AbortController;
use hoh_util_workspace_path::node_path::join;
fn file(data: Vec<u8>, name: &str) -> SaveFileAttachment {
    SaveFileAttachment {
        data,
        name: Some(name.into()),
    }
}
async fn empty_tmp(root: &str) {
    assert!(
        tokio::fs::read_dir(join(&[root, "tmp"]))
            .await
            .unwrap()
            .next_entry()
            .await
            .unwrap()
            .is_none()
    );
}
#[test]
fn keeps_ordinary_names_and_strips_client_paths_of_both_separator_styles() {
    for (name, expected) in [
        ("notes.pdf", "notes.pdf"),
        ("/home/user/data.csv", "data.csv"),
        ("C:\\Users\\me\\report.docx", "report.docx"),
    ] {
        assert_eq!(file_leaf_name(Some(name)), expected);
    }
}
#[test]
fn removes_control_characters_rewrites_windows_invalid_characters_and_bounds_utf8_length() {
    assert_eq!(file_leaf_name(Some("a\0b\u{1f}.txt")), "ab.txt");
    assert_eq!(
        file_leaf_name(Some("a<b>c:d\"e|f?g*h.txt")),
        "a_b_c_d_e_f_g_h.txt"
    );
    assert_eq!(
        file_leaf_name(Some(&format!("{}.bin", "x".repeat(300)))).len(),
        255
    );
    let multi = file_leaf_name(Some(&format!("{}.txt", "文".repeat(100))));
    assert!(multi.len() <= 255);
    assert!(!multi.ends_with('\u{fffd}'));
    let safe = file_leaf_name(Some(&format!("safe-{}😀", "x".repeat(248))));
    assert!(!safe.contains('\u{fffd}'));
}
#[test]
fn removes_windows_trailing_characters_and_protects_reserved_device_names() {
    for (name, expected) in [
        ("report. ", "report"),
        ("CON", "_CON"),
        ("com1.txt", "_com1.txt"),
        ("con .txt", "_con .txt"),
        ("com10.txt", "com10.txt"),
    ] {
        assert_eq!(file_leaf_name(Some(name)), expected);
    }
}
#[test]
fn falls_back_to_a_stable_name_for_absent_empty_and_dot_only_inputs() {
    for name in [None, Some(""), Some("   "), Some("."), Some("..")] {
        assert_eq!(file_leaf_name(name), "file");
    }
}
#[tokio::test]
async fn stores_the_exact_bytes_read_only_at_a_digest_and_name_path() {
    let home = tempfile::tempdir().unwrap();
    let root = root(&home);
    let data = vec![0, 1, 2, 250, 251, 252];
    let reference = save_file_verbatim(&root, &file(data.clone(), "blob.bin"))
        .await
        .unwrap();
    assert_eq!(
        reference,
        FileAttachmentRef {
            attachment_id: format!("sha256:{}", sha256(&data)).into(),
            name: "blob.bin".into(),
            bytes: 6
        }
    );
    let path = stored_file_path(&root, &reference).unwrap();
    assert!(path.ends_with(&join(&[&sha256(&data), "blob.bin"])));
    assert_eq!(tokio::fs::read(&path).await.unwrap(), data);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            tokio::fs::metadata(path)
                .await
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o400
        );
    }
}
#[tokio::test]
async fn accepts_a_zero_byte_file() {
    let home = tempfile::tempdir().unwrap();
    let root = root(&home);
    let reference = save_file_verbatim(&root, &file(vec![], "empty.txt"))
        .await
        .unwrap();
    assert_eq!(reference.bytes, 0);
    assert!(
        tokio::fs::read(stored_file_path(&root, &reference).unwrap())
            .await
            .unwrap()
            .is_empty()
    );
}
#[tokio::test]
async fn stores_sanitized_windows_reserved_and_multibyte_names() {
    let home = tempfile::tempdir().unwrap();
    let root = root(&home);
    let reserved = save_file_verbatim(&root, &file(vec![], "NUL.txt"))
        .await
        .unwrap();
    let multi = save_file_verbatim(&root, &file(vec![1], &"文".repeat(100)))
        .await
        .unwrap();
    assert_eq!(reserved.name, "_NUL.txt");
    assert!(multi.name.len() <= 255);
    assert!(
        tokio::fs::read(stored_file_path(&root, &reserved).unwrap())
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        tokio::fs::read(stored_file_path(&root, &multi).unwrap())
            .await
            .unwrap(),
        [1]
    );
}
#[tokio::test]
async fn deduplicates_identical_bytes_and_stores_distinct_names_beside_one_digest() {
    let home = tempfile::tempdir().unwrap();
    let root = root(&home);
    let data = vec![7, 7, 7];
    let first = save_file_verbatim(&root, &file(data.clone(), "a.txt"))
        .await
        .unwrap();
    let again = save_file_verbatim(&root, &file(data.clone(), "a.txt"))
        .await
        .unwrap();
    let renamed = save_file_verbatim(&root, &file(data, "b.txt"))
        .await
        .unwrap();
    assert_eq!(first, again);
    assert_eq!(first.attachment_id, renamed.attachment_id);
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        assert_eq!(
            tokio::fs::metadata(stored_file_path(&root, &first).unwrap())
                .await
                .unwrap()
                .ino(),
            tokio::fs::metadata(stored_file_path(&root, &renamed).unwrap())
                .await
                .unwrap()
                .ino()
        );
    }
    let mut dir = tokio::fs::read_dir(hoh_util_workspace_path::node_path::dirname(
        &stored_file_path(&root, &first).unwrap(),
    ))
    .await
    .unwrap();
    let mut names = vec![];
    while let Some(entry) = dir.next_entry().await.unwrap() {
        names.push(entry.file_name());
    }
    names.sort();
    assert_eq!(
        names,
        vec![std::ffi::OsString::from("a.txt"), "b.txt".into()]
    );
}
#[tokio::test]
async fn refuses_a_stored_object_whose_bytes_no_longer_match_the_digest() {
    let home = tempfile::tempdir().unwrap();
    let root = root(&home);
    let source = file(vec![1, 2, 3], "c.txt");
    let saved = save_file_verbatim(&root, &source).await.unwrap();
    let path = stored_file_path(&root, &saved).unwrap();
    permissions(&path, 0o600).await.unwrap();
    tokio::fs::write(path, [9, 9, 9]).await.unwrap();
    assert_code(
        save_file_verbatim(&root, &source).await,
        "ATTACHMENT_CORRUPT",
    );
}
#[tokio::test]
async fn rejects_a_conflicting_display_name_alias_and_wraps_alias_publication_failures() {
    let home = tempfile::tempdir().unwrap();
    let root = root(&home);
    let source = file(vec![1, 2, 3], "alias.bin");
    let saved = save_file_verbatim(&root, &source).await.unwrap();
    let path = stored_file_path(&root, &saved).unwrap();
    tokio::fs::remove_file(&path).await.unwrap();
    tokio::fs::write(path, [9, 9, 9]).await.unwrap();
    assert_code(
        save_file_verbatim(&root, &source).await,
        "ATTACHMENT_CORRUPT",
    );
    assert_code(
        publish_immutable_alias(
            &root,
            &join(&[&root, "missing-object"]),
            &join(&[&root, "files", "ff", "missing", "alias.bin"]),
            &"f".repeat(64),
        )
        .await,
        "ATTACHMENT_WRITE_FAILED",
    );
}
#[tokio::test]
async fn stores_ordered_chunks_without_materializing_one_aggregate_byte_array() {
    let home = tempfile::tempdir().unwrap();
    let root = root(&home);
    let source = SaveFileStreamAttachment {
        data: Box::pin(stream::iter([
            Ok(vec![0, 1]),
            Ok(vec![2, 250]),
            Ok(vec![251, 252]),
        ])),
        signal: None,
        name: Some("large.bin".into()),
    };
    let saved = save_file_stream_verbatim(&root, source).await.unwrap();
    let data = vec![0, 1, 2, 250, 251, 252];
    assert_eq!(
        saved.attachment_id.as_str(),
        format!("sha256:{}", sha256(&data))
    );
    assert_eq!(saved.bytes, 6);
    assert_eq!(saved.name, "large.bin");
    assert_eq!(
        tokio::fs::read(stored_file_path(&root, &saved).unwrap())
            .await
            .unwrap(),
        data
    );
}
#[tokio::test]
async fn removes_its_staging_file_when_cancellation_interrupts_the_source() {
    let home = tempfile::tempdir().unwrap();
    let root = root(&home);
    let controller = AbortController::new();
    let reason = SharedError::new(std::io::Error::other("upload cancelled"));
    let signal = controller.signal();
    let bytes = stream::iter([vec![1, 2], vec![3, 4]])
        .enumerate()
        .map(|(index, chunk)| {
            if index == 1 {
                controller.abort_with(reason.clone());
            }
            Ok(chunk)
        });
    let error = save_file_stream_verbatim(
        &root,
        SaveFileStreamAttachment {
            data: Box::pin(bytes),
            signal: Some(signal),
            name: Some("cancelled.bin".into()),
        },
    )
    .await
    .unwrap_err();
    assert!(SharedError::of(error.as_ref()).unwrap().ptr_eq(&reason));
    empty_tmp(&root).await;
}
#[tokio::test]
async fn wraps_source_failures_and_removes_the_staging_file() {
    let home = tempfile::tempdir().unwrap();
    let root = root(&home);
    let chunks: Vec<Result<Vec<u8>, BoxError>> = vec![
        Ok(vec![1, 2]),
        Err(std::io::Error::other("source failed").into()),
    ];
    assert_code(
        save_file_stream_verbatim(
            &root,
            SaveFileStreamAttachment {
                data: Box::pin(stream::iter(chunks)),
                signal: None,
                name: Some("failed.bin".into()),
            },
        )
        .await,
        "ATTACHMENT_WRITE_FAILED",
    );
    empty_tmp(&root).await;
}
#[tokio::test]
async fn returns_exact_bounded_chunks_and_accepts_an_empty_file() {
    let home = tempfile::tempdir().unwrap();
    let root = root(&home);
    let data: Vec<u8> = (0..65539).map(|i| (i % 251) as u8).collect();
    let saved = save_file_verbatim(&root, &file(data.clone(), "large.bin"))
        .await
        .unwrap();
    let mut stream = read_file_stream_verbatim(&root, &saved, None);
    let mut actual = vec![];
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.unwrap();
        assert!(chunk.len() <= 65536);
        actual.extend(chunk);
    }
    assert_eq!(actual, data);
    let empty = save_file_verbatim(&root, &file(vec![], "empty.bin"))
        .await
        .unwrap();
    assert!(
        collect(read_file_stream_verbatim(&root, &empty, None))
            .await
            .unwrap()
            .is_empty()
    );
}
#[tokio::test]
async fn rejects_invalid_missing_and_unreadable_references_with_storage_codes() {
    let home = tempfile::tempdir().unwrap();
    let root = root(&home);
    let reference = FileAttachmentRef {
        attachment_id: format!("sha256:{}", "c".repeat(64)).into(),
        name: "missing.bin".into(),
        bytes: 1,
    };
    assert_code(
        collect(read_file_stream_verbatim(
            &root,
            &FileAttachmentRef {
                name: "../escape".into(),
                ..reference.clone()
            },
            None,
        ))
        .await,
        "INVALID_ATTACHMENT_REF",
    );
    assert_code(
        collect(read_file_stream_verbatim(&root, &reference, None)).await,
        "ATTACHMENT_NOT_FOUND",
    );
    let saved = save_file_verbatim(&root, &file(vec![1], "unreadable.bin"))
        .await
        .unwrap();
    let path = stored_file_path(&root, &saved).unwrap();
    tokio::fs::remove_file(&path).await.unwrap();
    tokio::fs::create_dir(&path).await.unwrap();
    assert_code(
        collect(read_file_stream_verbatim(&root, &saved, None)).await,
        "ATTACHMENT_READ_FAILED",
    );
}
#[tokio::test]
async fn detects_changed_bytes_and_recorded_lengths() {
    let home = tempfile::tempdir().unwrap();
    let root = root(&home);
    let saved = save_file_verbatim(&root, &file(vec![1, 2, 3], "data.bin"))
        .await
        .unwrap();
    let path = stored_file_path(&root, &saved).unwrap();
    permissions(&path, 0o600).await.unwrap();
    tokio::fs::write(&path, [3, 2, 1]).await.unwrap();
    assert_code(
        collect(read_file_stream_verbatim(&root, &saved, None)).await,
        "ATTACHMENT_CORRUPT",
    );
    tokio::fs::write(&path, [1, 2, 3]).await.unwrap();
    assert_code(
        collect(read_file_stream_verbatim(
            &root,
            &FileAttachmentRef { bytes: 4, ..saved },
            None,
        ))
        .await,
        "ATTACHMENT_CORRUPT",
    );
}
#[tokio::test]
async fn preserves_caller_cancellation_before_and_during_a_read() {
    let home = tempfile::tempdir().unwrap();
    let root = root(&home);
    let saved = save_file_verbatim(&root, &file(vec![7; 1 << 17], "cancel.bin"))
        .await
        .unwrap();
    let before = AbortController::new();
    let reason = SharedError::new(std::io::Error::other("cancelled before read"));
    before.abort_with(reason.clone());
    let signal = before.signal();
    let error = collect(read_file_stream_verbatim(&root, &saved, Some(&signal)))
        .await
        .unwrap_err();
    assert!(SharedError::of(error.as_ref()).unwrap().ptr_eq(&reason));
    let during = AbortController::new();
    let signal = during.signal();
    let mut stream = read_file_stream_verbatim(&root, &saved, Some(&signal));
    assert!(!stream.next().await.unwrap().unwrap().is_empty());
    let reason = SharedError::new(std::io::Error::other("cancelled during read"));
    during.abort_with(reason.clone());
    let error = stream.next().await.unwrap().unwrap_err();
    assert!(SharedError::of(error.as_ref()).unwrap().ptr_eq(&reason));
}
#[test]
fn rejects_malformed_digests_and_unsanitized_names_before_deriving_a_path() {
    let good = FileAttachmentRef {
        attachment_id: format!("sha256:{}", "a".repeat(64)).into(),
        name: "ok.txt".into(),
        bytes: 1,
    };
    for bad in [
        FileAttachmentRef {
            attachment_id: "sha256:short".into(),
            ..good.clone()
        },
        FileAttachmentRef {
            name: "../escape.txt".into(),
            ..good.clone()
        },
        FileAttachmentRef {
            name: "nested/name.txt".into(),
            ..good.clone()
        },
    ] {
        assert_eq!(
            stored_file_path("/root", &bad).unwrap_err().code(),
            "INVALID_ATTACHMENT_REF"
        );
    }
    assert!(
        stored_file_path("/root", &good)
            .unwrap()
            .ends_with(&join(&[&"a".repeat(64), "ok.txt"]))
    );
}
