use crate::{store::*, test_support::*};
use hoh_attachment::*;
use hoh_error::SharedError;
use hoh_timeout::AbortController;
use hoh_util_workspace_path::node_path::{dirname, join};
use std::sync::{Arc, Mutex};

#[cfg(unix)]
#[tokio::test]
async fn syncs_every_object_ancestor_up_to_the_durable_boundary_before_returning() {
    let home = tempfile::tempdir().unwrap();
    let root = root(&home);
    let base = home.path().to_string_lossy().into_owned();
    let hash = sha256(&png());
    let objects = join(&[&root, "objects"]);
    let bucket = join(&[&objects, &hash[..2]]);
    let trace = Arc::new(Mutex::new(vec![]));
    SYNC_TRACE
        .scope(
            trace.clone(),
            save_image_file(&root, &input(png()), &limits(), &policy()),
        )
        .await
        .unwrap();
    let mut expected = vec![];
    let mut level = base.clone();
    while level != "/" {
        level = dirname(&level);
        expected.push(level.clone());
    }
    expected.extend([
        root.clone(),
        dirname(&root),
        base.clone(),
        objects.clone(),
        root.clone(),
        dirname(&root),
        base,
        bucket,
        objects,
    ]);
    assert_eq!(*trace.lock().unwrap(), expected);
}
#[tokio::test]
async fn creates_and_persists_a_missing_nested_home_directory_against_the_filesystem_root() {
    let home = tempfile::tempdir().unwrap();
    let root = join(&[&root(&home), "home", "attachments", "v1"]);
    let reference = save_image_file(&root, &input(png()), &limits(), &policy())
        .await
        .unwrap();
    let stored = read_image_file(&root, &reference, None).await.unwrap();
    assert_eq!(stored.r#ref, reference);
    assert_eq!(stored.data, png());
}
#[tokio::test]
async fn publishes_one_private_content_addressed_object_and_deduplicates_equal_bytes() {
    let home = tempfile::tempdir().unwrap();
    let root = root(&home);
    let mut source = input(png());
    source.name = Some("/private/tmp/pixel.png".into());
    let first = save_image_file(&root, &source, &limits(), &policy())
        .await
        .unwrap();
    let second = save_image_file(&root, &input(png()), &limits(), &policy())
        .await
        .unwrap();
    let hash = sha256(&png());
    let path = normalized_image_path(&root, &first).unwrap();
    assert_eq!(first.attachment_id.as_str(), format!("sha256:{hash}"));
    assert_eq!(first.name.as_deref(), Some("pixel.png"));
    assert_eq!(
        (first.bytes, first.width, first.height),
        (png().len() as u64, 1, 1)
    );
    assert_eq!(first.attachment_id, second.attachment_id);
    assert_eq!(tokio::fs::read(&path).await.unwrap(), png());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            tokio::fs::metadata(&path)
                .await
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o400
        );
        assert_eq!(
            tokio::fs::metadata(dirname(&path))
                .await
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }
    permissions(&path, 0o600).await.unwrap();
    save_image_file(&root, &input(png()), &limits(), &policy())
        .await
        .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            tokio::fs::metadata(&path)
                .await
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o400
        );
    }
    assert_eq!(
        read_image_file(&root, &first, None).await.unwrap().data,
        png()
    );
}
#[tokio::test]
async fn rejects_supplied_metadata_that_differs_from_stored_bytes() {
    let home = tempfile::tempdir().unwrap();
    let root = root(&home);
    let saved = save_image_file(&root, &input(png()), &limits(), &policy())
        .await
        .unwrap();
    let mut mismatches = vec![saved.clone(); 4];
    mismatches[0].media_type = ImageMediaType::Jpeg;
    mismatches[1].bytes += 1;
    mismatches[2].width = 2;
    mismatches[3].height = 2;
    for mismatch in mismatches {
        assert_code(
            read_image_file(&root, &mismatch, None).await,
            "ATTACHMENT_CORRUPT",
        );
    }
}
#[tokio::test]
async fn preserves_supplied_display_fields_while_verifying_stored_image_facts() {
    let home = tempfile::tempdir().unwrap();
    let root = root(&home);
    let mut saved = save_image_file(&root, &input(png()), &limits(), &policy())
        .await
        .unwrap();
    saved.name = Some("display.png".into());
    saved.original_dimensions = Some(ProjectedDimensions {
        width: 8,
        height: 8,
    });
    assert_eq!(
        read_image_file(&root, &saved, None).await.unwrap().r#ref,
        saved
    );
}
#[tokio::test]
async fn rejects_publication_when_the_supplied_digest_does_not_match_the_staged_bytes() {
    let home = tempfile::tempdir().unwrap();
    let root = root(&home);
    assert_code(
        publish_immutable_object(
            &root,
            &join(&[&root, "objects", "00", "mismatch"]),
            &[1],
            &"0".repeat(64),
        )
        .await,
        "ATTACHMENT_CORRUPT",
    );
    assert!(
        tokio::fs::read_dir(join(&[&root, "tmp"]))
            .await
            .unwrap()
            .next_entry()
            .await
            .unwrap()
            .is_none()
    );
}
#[cfg(windows)]
#[tokio::test]
async fn publishes_a_new_object_on_windows() {
    let home = tempfile::tempdir().unwrap();
    let root = root(&home);
    let reference = save_image_file(&root, &input(png()), &limits(), &policy())
        .await
        .unwrap();
    assert_eq!(
        read_image_file(&root, &reference, None).await.unwrap().data,
        png()
    );
}
#[tokio::test]
async fn stores_the_normalized_image_of_an_oversized_source_and_reads_it_back_verified() {
    let home = tempfile::tempdir().unwrap();
    let root = root(&home);
    let data = flat(4, 4, ::image::ImageFormat::Png, false).await;
    let mut source = input(data.clone());
    source.name = Some("big.png".into());
    let saved = save_image_file(
        &root,
        &source,
        &ImageAttachmentLimits {
            max_image_pixels: 64,
            ..limits()
        },
        &crate::NormalizationPolicy {
            max_dimension: 2,
            ..policy()
        },
    )
    .await
    .unwrap();
    assert_eq!(
        (saved.media_type, saved.width, saved.height),
        (ImageMediaType::Jpeg, 2, 2)
    );
    assert_eq!(saved.name.as_deref(), Some("big.png"));
    assert_eq!(
        saved.original_dimensions,
        Some(ProjectedDimensions {
            width: 4,
            height: 4
        })
    );
    assert_ne!(saved.bytes, data.len() as u64);
    let read = read_image_file(&root, &saved, None).await.unwrap();
    assert_eq!(saved.bytes, read.data.len() as u64);
    assert_eq!(
        saved.attachment_id.as_str(),
        format!("sha256:{}", sha256(&read.data))
    );
}
#[tokio::test]
async fn keeps_admitted_history_readable_after_deployment_limits_become_stricter() {
    let home = tempfile::tempdir().unwrap();
    let root = root(&home);
    let reference = save_image_file(&root, &input(png()), &limits(), &policy())
        .await
        .unwrap();
    assert_eq!(
        read_image_file(&root, &reference, None).await.unwrap().data,
        png()
    );
}
#[tokio::test]
async fn forwards_read_cancellation_to_the_filesystem_and_preserves_its_reason() {
    let home = tempfile::tempdir().unwrap();
    let root = root(&home);
    let reference = save_image_file(&root, &input(png()), &limits(), &policy())
        .await
        .unwrap();
    let controller = AbortController::new();
    let signal = controller.signal();
    let trace = Arc::new(Mutex::new(vec![]));
    assert_eq!(
        READ_SIGNALS
            .scope(
                trace.clone(),
                read_image_file(&root, &reference, Some(&signal))
            )
            .await
            .unwrap()
            .data,
        png()
    );
    assert_eq!(trace.lock().unwrap().len(), 1);
    let reason = SharedError::new(std::io::Error::other("attachment read cancelled"));
    controller.abort_with(reason.clone());
    assert!(trace.lock().unwrap()[0].reason().unwrap().ptr_eq(&reason));
    let failure = read_image_file(&root, &reference, Some(&signal))
        .await
        .unwrap_err();
    assert!(SharedError::of(failure.as_ref()).unwrap().ptr_eq(&reason));
}
#[tokio::test]
async fn rejects_malformed_bytes_mismatched_declarations_byte_limits_and_decoded_pixel_limits() {
    let home = tempfile::tempdir().unwrap();
    let root = root(&home);
    for data in [vec![], vec![1, 2, 3]] {
        assert_code(
            save_image_file(&root, &input(data), &limits(), &policy()).await,
            "INVALID_IMAGE",
        );
    }
    assert_code(
        save_image_file(
            &root,
            &SaveImageAttachment {
                media_type: ImageMediaType::Jpeg,
                ..input(png())
            },
            &limits(),
            &policy(),
        )
        .await,
        "IMAGE_TYPE_MISMATCH",
    );
    assert_code(
        save_image_file(
            &root,
            &input(png()),
            &ImageAttachmentLimits {
                max_image_bytes: 1,
                ..limits()
            },
            &policy(),
        )
        .await,
        "IMAGE_TOO_LARGE",
    );
    let wide = input(flat(5, 5, ::image::ImageFormat::Png, true).await);
    assert_code(
        save_image_file(&root, &wide, &limits(), &policy()).await,
        "IMAGE_TOO_MANY_PIXELS",
    );
    assert_code(
        save_image_file(
            &root,
            &wide,
            &ImageAttachmentLimits {
                max_image_pixels: 25,
                max_image_dimension: 4,
                ..limits()
            },
            &policy(),
        )
        .await,
        "IMAGE_DIMENSION_TOO_LARGE",
    );
    let unnamed = save_image_file(
        &root,
        &SaveImageAttachment {
            name: Some("\0".into()),
            ..input(png())
        },
        &limits(),
        &policy(),
    )
    .await
    .unwrap();
    assert!(unnamed.name.is_none());
}
#[tokio::test]
async fn fails_closed_when_an_object_is_missing_corrupted_or_addressed_by_an_invalid_reference() {
    let home = tempfile::tempdir().unwrap();
    let root = root(&home);
    let saved = save_image_file(&root, &input(png()), &limits(), &policy())
        .await
        .unwrap();
    let path = normalized_image_path(&root, &saved).unwrap();
    permissions(&path, 0o600).await.unwrap();
    tokio::fs::write(&path, [1, 2, 3]).await.unwrap();
    assert_code(
        read_image_file(&root, &saved, None).await,
        "ATTACHMENT_CORRUPT",
    );
    let invalid = ImageAttachmentRef {
        attachment_id: "bad".into(),
        ..saved.clone()
    };
    assert_code(
        read_image_file(&root, &invalid, None).await,
        "INVALID_ATTACHMENT_REF",
    );
    let missing = tempfile::tempdir().unwrap();
    assert_code(
        read_image_file(&crate::test_support::root(&missing), &saved, None).await,
        "ATTACHMENT_NOT_FOUND",
    );
    tokio::fs::remove_file(&path).await.unwrap();
    tokio::fs::create_dir(&path).await.unwrap();
    let unreadable = assert_code(
        read_image_file(&root, &saved, None).await,
        "ATTACHMENT_READ_FAILED",
    );
    #[cfg(unix)]
    assert_eq!(
        unreadable.source().unwrap().to_string(),
        "EISDIR: illegal operation on a directory, read"
    );
    #[cfg(not(unix))]
    let _ = unreadable;
}
#[tokio::test]
async fn rejects_conflicting_existing_objects_and_reference_metadata_mismatches() {
    let home = tempfile::tempdir().unwrap();
    let root = root(&home);
    let hash = sha256(&png());
    let path = join(&[&root, "objects", &hash[..2], &hash]);
    tokio::fs::create_dir_all(dirname(&path)).await.unwrap();
    tokio::fs::write(&path, [1, 2, 3]).await.unwrap();
    assert_code(
        save_image_file(&root, &input(png()), &limits(), &policy()).await,
        "ATTACHMENT_CORRUPT",
    );
    tokio::fs::write(&path, png()).await.unwrap();
    let mut saved = save_image_file(&root, &input(png()), &limits(), &policy())
        .await
        .unwrap();
    saved.width += 1;
    assert_code(
        read_image_file(&root, &saved, None).await,
        "ATTACHMENT_CORRUPT",
    );
}
#[tokio::test]
async fn maps_unexpected_publication_failures_to_a_stable_storage_error() {
    let home = tempfile::tempdir().unwrap();
    let root = root(&home);
    let hash = sha256(&png());
    tokio::fs::create_dir_all(join(&[&root, "objects", &hash[..2], &hash]))
        .await
        .unwrap();
    assert_code(
        save_image_file(&root, &input(png()), &limits(), &policy()).await,
        "ATTACHMENT_WRITE_FAILED",
    );
}
#[tokio::test]
async fn rejects_prepared_bytes_that_no_longer_match_their_content_addressed_reference() {
    let home = tempfile::tempdir().unwrap();
    let root = root(&home);
    let mut prepared = prepare_image_file(&input(png()), &limits(), &policy())
        .await
        .unwrap();
    prepared.data.push(0);
    assert_code(
        commit_prepared_image_file(&root, &prepared).await,
        "ATTACHMENT_CORRUPT",
    );
}
